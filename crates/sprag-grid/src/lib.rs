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
/// Rows that fall in the scrollback region are **text-only** — the R16
/// scrollback model retains trailing-trimmed text, not full cells — so scrolled
/// history renders in the default fg/bg with no attributes (and wide clusters as
/// a single cell: the head/trailer split is not recoverable from text); rows
/// still in the visible region keep their exact cells. The cursor is omitted
/// while scrolled (it lives in the live region below the view). `offset_lines`
/// is clamped to the retained scrollback depth.
#[must_use]
pub fn project_scrolled(screen: &Screen, offset_lines: usize) -> GridBuffer {
    if offset_lines == 0 {
        return project(screen);
    }
    let cols = screen.cols();
    let rows = screen.rows();
    let scrollback: Vec<&str> = screen.scrollback_rows().collect();
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
            history_row(scrollback[logical], cols)
        } else {
            project_row(screen, (logical - scrollback_len) as u16, cols)
        };
        buffer = buffer.with_row(display, cells);
    }
    // No cursor while scrolled; the screen kind matches the live screen.
    buffer.with_screen(screen_kind(screen.screen_kind()))
}

/// Build a scrollback (text-only) row's `TermCell`s: each `char` as a
/// default-color narrow cell, padded with blanks / truncated to `cols`. The
/// text model has lost the cell structure, so this is an approximation of the
/// original row (the live grid is exact); the common ASCII case is faithful.
fn history_row(text: &str, cols: u16) -> Vec<TermCell> {
    let mut out = Vec::with_capacity(cols as usize);
    for ch in text.chars() {
        if out.len() >= cols as usize {
            break;
        }
        out.push(TermCell::new(
            ch.to_string(),
            TermColor::Default,
            TermColor::Default,
        ));
    }
    while out.len() < cols as usize {
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
                let head = term_cell(cell).wide();
                let trailer = head.trailer();
                out.push(head);
                out.push(trailer);
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
    TermCell::new(cell.cluster.clone(), term_color(cell.fg), term_color(cell.bg))
        .with_attrs(cell_attrs(cell.attrs))
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

    #[test]
    fn projects_wide_head_and_trailer() {
        let screen = screen_from("世".as_bytes(), 6, 1);
        let buffer = project(&screen);
        assert_eq!(buffer.cell(0, 0).unwrap().width, pinion_core::CellWidth::Wide);
        assert_eq!(buffer.cell(1, 0).unwrap().width, pinion_core::CellWidth::Trailer);
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
        assert!(scrolled.cursor().visible, "the live cursor is present after the clamp");
    }
}
