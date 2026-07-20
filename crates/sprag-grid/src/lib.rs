//! sprag-grid — project a [`sprag_vt::Screen`] into a pinion `GridBuffer`.
//!
//! DESIGN.md §3: sprag (the producer) owns the authoritative terminal
//! state; pinion holds a retained projection. This crate is the
//! one-directional adapter — a fresh `GridBuffer` per frame, mapping the
//! port's cells/cursor/screen-kind/damage onto pinion's data model.
//! Because both sides model the same axes, this is a flat mapping rather
//! than a translation.

use pinion_core::style::Color as PinColor;
use pinion_core::{
    CellAttrs, CursorShape as PinCursorShape, GridBuffer, GridCursor, ScreenKind as PinScreenKind,
    TermCell, TermColor,
};
use sprag_vt::{Attrs, Cell, Color, CursorShape, Screen, ScreenKind, Width};

/// Project a screen into a fresh pinion `GridBuffer`.
///
/// pinion replaces the node's buffer wholesale each frame (no per-cell
/// mutation), so a new buffer per call is the intended shape.
#[must_use]
pub fn project(screen: &Screen) -> GridBuffer {
    let cols = screen.cols();
    let rows = screen.rows();
    let mut buffer = GridBuffer::new(cols, rows);

    for row in 0..rows {
        buffer = buffer.with_row(row, project_row(screen, row, cols));
        if let Some(generation) = screen.row_generation(row) {
            buffer = buffer.with_row_generation(row, generation);
        }
    }

    let cursor = screen.cursor();
    buffer = buffer.with_cursor(GridCursor::new(
        cursor.col,
        cursor.row,
        cursor_shape(cursor.shape),
        cursor.visible,
    ));
    buffer.with_screen(screen_kind(screen.screen_kind()))
}

/// Project a screen into a `GridBuffer` scrolled up by `offset_lines` rows of
/// history. `offset_lines == 0` is the live view, byte-identical to [`project`].
///
/// A positive offset shows the pane's scrollback: the displayed window of
/// `screen.rows()` logical rows ends `offset_lines` rows above the live bottom.
/// Rows in the scrollback region are projected from their **stored styled cells**
/// ([`Screen::scrollback_cells`]) via `project_glyph_row`, so scrolled history
/// keeps its original fg/bg/attrs (and wide clusters their head/trailer split) —
/// identical styling to the live region. Rows still in the visible region keep
/// their exact cells. The cursor is omitted while scrolled (it lives in the live
/// region below the view). `offset_lines` is clamped to the retained scrollback
/// depth.
#[must_use]
pub fn project_scrolled(screen: &Screen, offset_lines: usize) -> GridBuffer {
    if offset_lines == 0 {
        return project(screen);
    }
    let cols = screen.cols();
    let rows = screen.rows();
    let scrollback: Vec<&[Cell]> = screen.scrollback_cells().collect();
    let scrollback_len = scrollback.len();
    let offset = offset_lines.min(scrollback_len);
    if offset == 0 {
        // A stale positive offset against now-empty scrollback (the screen
        // cleared its history, or switched to the alternate screen) IS the live
        // view — return it with the cursor, not a cursor-less window.
        return project(screen);
    }
    // First displayed row's index into the logical [scrollback .. visible]
    // sequence: the window of `rows` rows ends `offset` above the live bottom
    // (offset <= scrollback_len, so this never underflows).
    let top = scrollback_len - offset;

    let mut buffer = GridBuffer::new(cols, rows);
    for display in 0..rows {
        let logical = top + display as usize;
        let cells = if logical < scrollback_len {
            project_glyph_row(scrollback[logical], cols)
        } else {
            project_row(screen, (logical - scrollback_len) as u16, cols)
        };
        buffer = buffer.with_row(display, cells);
    }
    // No cursor while scrolled; the screen kind matches the live screen.
    buffer.with_screen(screen_kind(screen.screen_kind()))
}

/// Overlay an in-progress IME preedit (composition) string onto `buffer` at its
/// cursor, drawn underlined to mark it as composing.
///
/// **This is the one source of the rationale** the host/GUI seams cross-reference:
/// under winit + XIM the platform IME does **not** paint the in-progress preedit
/// over-the-spot; it emits an `Ime::Preedit` for the application to render. So a
/// terminal must draw the half-composed syllable itself — this overlay is that
/// rendering, the visual feedback that makes Hangul/CJK composition visible
/// before commit. It is display-only: the preedit never reaches the PTY — only a
/// committed [`CompositionEvent::Commit`](pinion_core::CompositionEvent) writes
/// (via the host text seam), at which point the committed glyphs arrive through
/// the PTY and the overlay clears.
///
/// Width comes from the producer's authority ([`sprag_vt::char_columns`]) — the
/// same model the emulator prints with — not a second width computation, so a
/// composing wide (CJK) syllable occupies the two cells (head + trailer) its
/// committed form will. A wide head whose trailer would fall off the row's right
/// edge is clipped *whole* (never emitted as a malformed Narrow-tagged wide
/// cell), a narrow cluster past the edge is clipped, and zero-width combining
/// marks are skipped (the emulator's documented grapheme-cluster gap).
///
/// No-ops (returns `buffer` unchanged) when the preedit is empty (no active
/// composition) **or** the buffer has no visible cursor — the cursor is the
/// compose anchor, so a scrolled-back history window (cursor-less per
/// [`project_scrolled`]) or a DECTCEM-hidden cursor has nowhere to anchor. This
/// is the single "no cursor ⇒ no overlay" gate; callers need no offset check.
#[must_use]
pub fn overlay_preedit(buffer: GridBuffer, preedit: &str) -> GridBuffer {
    let cursor = buffer.cursor();
    if preedit.is_empty() || !cursor.visible {
        return buffer;
    }
    let cols = buffer.cols();
    let row = cursor.row;
    if cols == 0 || row >= buffer.rows() {
        return buffer;
    }
    // Copy the cursor row, then splice the preedit in starting at the cursor
    // column. `with_row` rewrites the whole row, so the copy preserves the
    // committed cells to the left/right of the composition. Every (col, row)
    // here is in bounds (col < cols, row < rows checked above), so `cell` is
    // always `Some` — a `None` would be an internal `GridBuffer` invariant break.
    let cols_usize = usize::from(cols);
    let mut cells: Vec<TermCell> = (0..cols)
        .map(|col| {
            buffer
                .cell(col, row)
                .cloned()
                .expect("col < cols and row < rows hold above")
        })
        .collect();
    let mut col = usize::from(cursor.col);
    for ch in preedit.chars() {
        match sprag_vt::char_columns(ch) {
            // Combining mark: merged into the previous cell by the emulator; the
            // overlay has no previous-cell to merge into, so skip (the gap).
            0 => continue,
            // Wide (CJK) head needs its column AND the trailer column; clip the
            // whole head if the trailer would run off the edge (no lone wide head).
            2 => {
                if col + 1 >= cols_usize {
                    break;
                }
                let wide = preedit_cell(ch).wide();
                let trailer = wide.trailer();
                cells[col] = wide;
                cells[col + 1] = trailer;
                col += 2;
            }
            // Narrow cluster (1 column; any unexpected >2 width also renders narrow).
            _ => {
                if col >= cols_usize {
                    break;
                }
                cells[col] = preedit_cell(ch);
                col += 1;
            }
        }
    }
    buffer.with_row(row, cells)
}

/// Overlay a mouse text selection onto `buffer` by INVERTING (toggling `reverse`)
/// each selected cell — the xterm-idiomatic selection highlight (a cell already in
/// reverse video reads normal under the band, exactly as a real terminal inverts the
/// selected region). `start` and `end` are `(col, row)` in the buffer's VISIBLE grid,
/// in reading (row-major) order (`start <= end`), both INCLUSIVE — a linear / stream
/// selection: the first row runs from `start.col` to the row end, whole middle rows,
/// the last row up to `end.col`. Rows / columns past the buffer are clamped; the caller
/// gates out an empty span (a zero-width click selects nothing), so this always inverts
/// at least the span it is given.
///
/// Like [`overlay_preedit`] this rewrites only the affected rows wholesale
/// ([`GridBuffer::with_row`]) — the display-only projection is rebuilt every frame, so
/// mutating the projected buffer never touches the producer's authoritative cells.
#[must_use]
pub fn overlay_selection(buffer: GridBuffer, start: (u16, u16), end: (u16, u16)) -> GridBuffer {
    let cols = buffer.cols();
    let rows = buffer.rows();
    if cols == 0 || rows == 0 {
        return buffer;
    }
    let (start_col, start_row) = start;
    let (end_col, end_row) = end;
    let last_row = end_row.min(rows - 1);
    let mut buffer = buffer;
    let mut row = start_row;
    while row <= last_row {
        let first = if row == start_row { start_col } else { 0 };
        let last_incl = if row == end_row { end_col } else { cols - 1 }.min(cols - 1);
        if first <= last_incl {
            let cells: Vec<TermCell> = (0..cols)
                .map(|col| {
                    let cell = buffer
                        .cell(col, row)
                        .cloned()
                        .unwrap_or_else(TermCell::blank);
                    if col >= first && col <= last_incl {
                        let attrs = cell.attrs;
                        cell.with_attrs(attrs.with_reverse(!attrs.reverse))
                    } else {
                        cell
                    }
                })
                .collect();
            buffer = buffer.with_row(row, cells);
        }
        row += 1;
    }
    buffer
}

/// One preedit cell: the char in default colors, underlined to mark an
/// in-progress composition. The grid cursor (a block at the compose position)
/// highlights the active cell; the underline distinguishes the rest of the
/// composing run from committed text.
fn preedit_cell(ch: char) -> TermCell {
    TermCell::new(ch.to_string(), TermColor::Default, TermColor::Default)
        .with_attrs(CellAttrs::empty().with_underline(true))
}

/// Build a scrolled-back (history) row's `TermCell`s from its STORED cells,
/// preserving fg/bg/attrs — so scrollback paints in its original colors, not flat
/// plain text. Wide heads expand into pinion's head + trailer pair (the same shape
/// [`project_row`] gives the live grid). Both scrollback push paths store a row as
/// head+trailer pairs (`scroll_region_up` copies the live grid row; `reflowed` Pass 2
/// regenerates the trailer via `Cell::trailer_for`), so a stored `Width::Trailer`
/// is REDUNDANT — its head already emitted the pair — and is skipped here. The one
/// trailerless input is the degenerate `cols == 1` lone wide head (the emulator
/// stores a head with no room for a trailer); it lands in the `Width::Wide`
/// no-room arm below and renders narrow (matching `project_row`'s edge clip).
/// Padded with blanks / truncated to `cols`.
fn project_glyph_row(glyphs: &[Cell], cols: u16) -> Vec<TermCell> {
    let ncols = cols as usize;
    let mut out = Vec::with_capacity(ncols);
    for cell in glyphs {
        let col = out.len();
        if col >= ncols {
            break;
        }
        match cell.width {
            Width::Wide if col + 1 < ncols => push_wide_pair(&mut out, cell),
            Width::Trailer => {}
            _ => out.push(term_cell(cell)),
        }
    }
    while out.len() < ncols {
        out.push(TermCell::blank());
    }
    out
}

/// Build one row's `TermCell`s, expanding wide heads into pinion's
/// head + trailer pair (DESIGN.md §3: producer determines width).
fn project_row(screen: &Screen, row: u16, cols: u16) -> Vec<TermCell> {
    let mut out = Vec::with_capacity(cols as usize);
    let mut col = 0;
    while col < cols {
        let Some(cell) = screen.cell(col, row) else {
            break;
        };
        match cell.width {
            Width::Wide if col + 1 < cols => {
                push_wide_pair(&mut out, cell);
                col += 2;
            }
            // An orphan trailer means the head was clipped at the edge;
            // emit a blank so the column count stays exact.
            Width::Trailer => {
                out.push(TermCell::blank());
                col += 1;
            }
            _ => {
                out.push(term_cell(cell));
                col += 1;
            }
        }
    }
    out
}

fn term_cell(cell: &Cell) -> TermCell {
    TermCell::new(
        cell.cluster.clone(),
        term_color(cell.fg),
        term_color(cell.bg),
    )
    .with_attrs(cell_attrs(cell.attrs))
}

/// Push a wide cluster as pinion's head + trailer pair (DESIGN.md §3: producer
/// determines width). Shared by the live-grid [`project_row`] and the history
/// [`project_glyph_row`], which differ in iteration but emit wide cells the same way.
fn push_wide_pair(out: &mut Vec<TermCell>, cell: &Cell) {
    let head = term_cell(cell).wide();
    let trailer = head.trailer();
    out.push(head);
    out.push(trailer);
}

fn term_color(color: Color) -> TermColor {
    match color {
        Color::Default => TermColor::Default,
        Color::Indexed(index) => TermColor::Indexed(index),
        Color::Rgb(rgb) => TermColor::Rgb(PinColor::rgb(rgb.r, rgb.g, rgb.b)),
    }
}

fn cell_attrs(attrs: Attrs) -> CellAttrs {
    CellAttrs::empty()
        .with_bold(attrs.bold)
        .with_dim(attrs.dim)
        .with_italic(attrs.italic)
        .with_underline(attrs.underline)
        .with_blink(attrs.blink)
        .with_reverse(attrs.reverse)
        .with_hidden(attrs.hidden)
        .with_strikethrough(attrs.strikethrough)
}

fn cursor_shape(shape: CursorShape) -> PinCursorShape {
    match shape {
        CursorShape::Block => PinCursorShape::Block,
        CursorShape::Bar => PinCursorShape::Bar,
        CursorShape::Underline => PinCursorShape::Underline,
    }
}

fn screen_kind(kind: ScreenKind) -> PinScreenKind {
    match kind {
        ScreenKind::Main => PinScreenKind::Main,
        ScreenKind::Alternate => PinScreenKind::Alternate,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sprag_vt::{Emulator, VtPort};

    fn screen_from(bytes: &[u8], cols: u16, rows: u16) -> Screen {
        let mut em = Emulator::new(cols, rows);
        em.advance(bytes);
        em.screen().clone()
    }

    #[test]
    fn projects_dimensions_and_text() {
        let screen = screen_from(b"hi", 10, 2);
        let buffer = project(&screen);
        assert_eq!(buffer.cols(), 10);
        assert_eq!(buffer.rows(), 2);
        assert_eq!(buffer.cell(0, 0).unwrap().cluster, "h");
        assert_eq!(buffer.cell(1, 0).unwrap().cluster, "i");
    }

    #[test]
    fn projects_indexed_color_and_bold() {
        let screen = screen_from(b"\x1b[1;31mA", 4, 1);
        let buffer = project(&screen);
        let cell = buffer.cell(0, 0).unwrap();
        assert_eq!(cell.fg, TermColor::Indexed(1));
        assert!(cell.attrs.bold);
    }

    /// A colored row scrolled off the top keeps its fg/attrs in the scrollback
    /// projection — the cell-based scrollback (was flattened to plain text, so
    /// history rendered colorless while the live prompt stayed colored).
    #[test]
    fn scrolled_off_row_keeps_color_and_attrs() {
        // 1-row screen: bold-red "A", then a newline scrolls it into scrollback.
        let screen = screen_from(b"\x1b[1;31mA\r\n", 4, 1);
        assert_eq!(screen.scrollback_len(), 1, "the A row scrolled off");
        let buf = project_scrolled(&screen, 1);
        let cell = buf.cell(0, 0).unwrap();
        assert_eq!(cell.cluster, "A");
        assert_eq!(cell.fg, TermColor::Indexed(1), "scrollback keeps fg color");
        assert!(cell.attrs.bold, "scrollback keeps bold");
    }

    #[test]
    fn projects_wide_head_and_trailer() {
        let screen = screen_from("世".as_bytes(), 6, 1);
        let buffer = project(&screen);
        assert_eq!(
            buffer.cell(0, 0).unwrap().width,
            pinion_core::CellWidth::Wide
        );
        assert_eq!(
            buffer.cell(1, 0).unwrap().width,
            pinion_core::CellWidth::Trailer
        );
    }

    #[test]
    fn projects_cursor_position() {
        let screen = screen_from(b"abc", 10, 1);
        let buffer = project(&screen);
        assert_eq!(buffer.cursor().col, 3);
        assert_eq!(buffer.cursor().row, 0);
    }

    /// A 2-row screen fed 5 lines scrolls 3 off the top into scrollback;
    /// `project_scrolled` windows over [scrollback .. visible].
    #[test]
    fn project_scrolled_windows_history() {
        // 5 lines on a 2-row screen: rows "d","e" visible, "a","b","c" scrolled.
        let screen = screen_from(b"a\r\nb\r\nc\r\nd\r\ne", 4, 2);
        assert_eq!(screen.scrollback_len(), 3);
        let row0 = |buf: &GridBuffer| {
            (0..buf.cols())
                .filter_map(|c| buf.cell(c, 0).map(|cell| cell.cluster.clone()))
                .collect::<String>()
                .trim_end()
                .to_owned()
        };
        let row1 = |buf: &GridBuffer| {
            (0..buf.cols())
                .filter_map(|c| buf.cell(c, 1).map(|cell| cell.cluster.clone()))
                .collect::<String>()
                .trim_end()
                .to_owned()
        };
        // offset 0 == live (rows d, e), identical to project().
        let live = project_scrolled(&screen, 0);
        assert_eq!((row0(&live), row1(&live)), ("d".into(), "e".into()));
        // offset 1: one scrollback line ("c") on top, visible "d" below.
        let up1 = project_scrolled(&screen, 1);
        assert_eq!((row0(&up1), row1(&up1)), ("c".into(), "d".into()));
        // offset 3: top of history ("a", "b").
        let up3 = project_scrolled(&screen, 3);
        assert_eq!((row0(&up3), row1(&up3)), ("a".into(), "b".into()));
        // Clamp: a larger offset cannot scroll past the oldest line.
        let up99 = project_scrolled(&screen, 99);
        assert_eq!((row0(&up99), row1(&up99)), ("a".into(), "b".into()));
    }

    /// A stale positive offset against empty scrollback (history cleared, or
    /// alt-screen) clamps to the live view and KEEPS the cursor — not a
    /// cursor-less window.
    #[test]
    fn project_scrolled_clamps_stale_offset_to_live_with_cursor() {
        let screen = screen_from(b"abc", 10, 2);
        assert_eq!(screen.scrollback_len(), 0);
        let scrolled = project_scrolled(&screen, 7); // stale offset, no history
        let live = project(&screen);
        assert_eq!(scrolled.cursor().col, live.cursor().col);
        assert!(
            scrolled.cursor().visible,
            "the live cursor is present after the clamp"
        );
    }

    /// A narrow preedit is spliced in at the cursor (col 2 after "ab"),
    /// underlined, leaving the committed cells to its left intact.
    #[test]
    fn overlay_preedit_underlines_narrow_text_at_the_cursor() {
        let screen = screen_from(b"ab", 10, 1);
        let buffer = overlay_preedit(project(&screen), "x");
        assert_eq!(
            buffer.cell(0, 0).unwrap().cluster,
            "a",
            "committed text is preserved"
        );
        assert_eq!(buffer.cell(1, 0).unwrap().cluster, "b");
        let composed = buffer.cell(2, 0).unwrap(); // cursor sat at col 2
        assert_eq!(composed.cluster, "x");
        assert!(
            composed.attrs.underline,
            "the preedit is underlined (composing marker)"
        );
    }

    /// A wide (Hangul) preedit syllable expands to pinion's head + trailer pair,
    /// occupying the two cells its committed form will.
    #[test]
    fn overlay_preedit_expands_a_wide_syllable_to_head_and_trailer() {
        let screen = screen_from(b"ab", 10, 1);
        let buffer = overlay_preedit(project(&screen), "한");
        let head = buffer.cell(2, 0).unwrap();
        assert_eq!(head.cluster, "한");
        assert_eq!(head.width, pinion_core::CellWidth::Wide);
        assert!(head.attrs.underline);
        assert_eq!(
            buffer.cell(3, 0).unwrap().width,
            pinion_core::CellWidth::Trailer
        );
    }

    /// An empty preedit (no active composition) is a no-op — the live view is
    /// byte-identical to the bare projection.
    #[test]
    fn overlay_preedit_empty_is_a_no_op() {
        let screen = screen_from(b"ab", 10, 1);
        let plain = project(&screen);
        let overlaid = overlay_preedit(project(&screen), "");
        for col in 0..plain.cols() {
            assert_eq!(
                plain.cell(col, 0).unwrap().cluster,
                overlaid.cell(col, 0).unwrap().cluster
            );
        }
    }

    /// A wide preedit whose trailer would run off the row edge is clipped WHOLE —
    /// never written as a malformed Narrow-tagged wide cell (the M2 fix).
    #[test]
    fn overlay_preedit_clips_a_wide_syllable_at_the_row_edge() {
        let screen = screen_from(b"abc", 4, 1); // 4 cols, cursor lands on the last column (3)
        assert_eq!(project(&screen).cursor().col, 3);
        let buffer = overlay_preedit(project(&screen), "한");
        let last = buffer.cell(3, 0).unwrap();
        assert_ne!(
            last.cluster, "한",
            "the wide head is clipped at the edge, not written"
        );
        assert_ne!(
            last.width,
            pinion_core::CellWidth::Wide,
            "no lone Narrow-tagged wide head"
        );
    }

    /// The `project_scrolled` + `overlay_preedit` COMPOSITION a display client runs:
    /// a scrolled history window drops the cursor, so the overlay self-gates off and
    /// the preedit appears NOWHERE — while the live window (offset 0) shows it. This
    /// is the end-to-end fact the GUI's `build_pane_scene` relies on (topology B: the
    /// host projects, the client overlays); it used to live in a sprag-host test over
    /// the removed all-in-one pane builder, and belongs here with the two primitives.
    #[test]
    fn overlay_preedit_no_op_on_a_scrolled_history_window() {
        let screen = screen_from(b"a\r\nb\r\nc\r\nd\r\ne", 4, 2); // 3 rows into history
        assert_eq!(screen.scrollback_len(), 3);
        let contains_han = |buf: &GridBuffer| {
            (0..buf.cols())
                .any(|c| (0..buf.rows()).any(|r| buf.cell(c, r).is_some_and(|x| x.cluster == "한")))
        };
        // Live (offset 0): the composition overlays the preedit at the cursor.
        let live = overlay_preedit(project_scrolled(&screen, 0), "한");
        assert!(contains_han(&live), "the live window shows the preedit");
        // Scrolled (offset 1): the cursor is dropped, so the overlay self-gates off.
        let scrolled = overlay_preedit(project_scrolled(&screen, 1), "한");
        assert!(
            !contains_han(&scrolled),
            "a scrolled history window shows no preedit"
        );
    }

    /// No visible cursor (a scrolled history window, or a DECTCEM-hidden cursor)
    /// is no compose anchor — the overlay is a no-op (the single S1 gate).
    #[test]
    fn overlay_preedit_no_op_without_a_visible_cursor() {
        let screen = screen_from(b"ab", 10, 1);
        let hidden =
            project(&screen).with_cursor(GridCursor::new(2, 0, PinCursorShape::Block, false));
        let out = overlay_preedit(hidden, "x");
        assert_ne!(
            out.cell(2, 0).unwrap().cluster,
            "x",
            "no overlay without a visible cursor"
        );
    }

    /// A same-row selection inverts exactly the selected columns; a cell already in
    /// reverse video toggles back to normal under the band (xterm-style invert).
    #[test]
    fn overlay_selection_inverts_the_selected_span() {
        let screen = screen_from(b"abcdef", 10, 1);
        let buf = overlay_selection(project(&screen), (1, 0), (3, 0)); // cols 1..=3
        assert!(!buf.cell(0, 0).unwrap().attrs.reverse, "col 0 outside");
        assert!(buf.cell(1, 0).unwrap().attrs.reverse, "col 1 selected");
        assert!(buf.cell(3, 0).unwrap().attrs.reverse, "col 3 selected");
        assert!(!buf.cell(4, 0).unwrap().attrs.reverse, "col 4 outside");
        // SGR 7 (reverse) cell toggles back to normal inside the selection.
        let rev = screen_from(b"\x1b[7mX", 4, 1);
        assert!(project(&rev).cell(0, 0).unwrap().attrs.reverse);
        let sel = overlay_selection(project(&rev), (0, 0), (0, 0));
        assert!(
            !sel.cell(0, 0).unwrap().attrs.reverse,
            "a reverse cell inverts to normal under the selection"
        );
    }

    /// A multi-row selection is linear: first row from the start col to the row end,
    /// whole middle rows, the last row up to the end col.
    #[test]
    fn overlay_selection_spans_rows_linearly() {
        let screen = screen_from(b"aaaa\r\nbbbb\r\ncccc", 4, 3);
        let buf = overlay_selection(project(&screen), (2, 0), (1, 2));
        // Row 0: cols 2,3 selected; 0,1 not.
        assert!(!buf.cell(1, 0).unwrap().attrs.reverse);
        assert!(buf.cell(2, 0).unwrap().attrs.reverse);
        // Row 1 (middle): fully selected.
        assert!(buf.cell(0, 1).unwrap().attrs.reverse);
        assert!(buf.cell(3, 1).unwrap().attrs.reverse);
        // Row 2: cols 0,1 selected; 2,3 not.
        assert!(buf.cell(1, 2).unwrap().attrs.reverse);
        assert!(!buf.cell(2, 2).unwrap().attrs.reverse);
    }
}
