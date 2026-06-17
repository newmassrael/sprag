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
}
