//! The slice-2 verification: a VT battery through the REAL chain, asserted on cells.
//!
//! `escape sequences -> sprag_vt::Emulator -> sprag_grid::project -> GridBuffer ->
//! sprag_tui::pane_changes -> termwiz Surface`, with no terminal and no daemon anywhere in it.
//!
//! # Why the battery is driven and not hand-built
//!
//! [`crate::paint`](../src/paint.rs)'s own unit tests build `GridBuffer`s directly, which proves
//! the mapping is correct for the cells THEY describe. That is a weaker claim than it looks: a
//! hand-built cell is one I invented, and the mapping only matters for cells a pane actually
//! produces. So this file writes what a program writes — SGR sequences, CJK text, an OSC 8
//! hyperlink — and lets sprag's own emulator and projection decide what the cells are. If the
//! emulator ever changed what it emits for `ESC [ 4:3 m`, these tests would move with it and the
//! unit tests would not.
//!
//! It is the same "assert the cells, not a screenshot" discipline `sprag-grid`'s own tests use,
//! one layer further down the pipe.

use pinion_core::GridBuffer;
use sprag_grid::project;
use sprag_tui::{Rect, cursor_changes, pane_changes};
use sprag_vt::{Emulator, VtPort};
use termwiz::cell::{Blink, Intensity, Underline};
use termwiz::color::{ColorAttribute, SrgbaTuple};
use termwiz::surface::Surface;

/// Feed `bytes` to a `cols x rows` emulator and project what it made — the client's-eye view of a
/// pane that has just printed something.
fn pane(bytes: &[u8], cols: u16, rows: u16) -> GridBuffer {
    let mut emulator = Emulator::new(cols, rows);
    emulator.advance(bytes);
    project(VtPort::screen(&emulator), VtPort::palette(&emulator))
}

/// Paint a projected pane onto a surface of its own size, as the sole focused pane — the
/// single-pane composition the client makes when a session has one pane.
fn painted(grid: &GridBuffer) -> Surface {
    let area = Rect::screen(grid.cols(), grid.rows());
    let mut surface = Surface::new(usize::from(grid.cols()), usize::from(grid.rows()));
    surface.add_changes(pane_changes(grid, area, (0, 0)));
    surface.add_changes(cursor_changes(grid, area, (0, 0)));
    surface
}

/// xterm's ANSI red — what index 1 resolves to through the projection's palette, and therefore
/// what a client is handed for `ESC [ 31 m`. Written as a truecolor because that is the ONLY form
/// that crosses the wire: the projection resolves indexed colours at the producer so an OSC 4
/// palette change restains cells printed before it.
fn xterm_red() -> ColorAttribute {
    ColorAttribute::TrueColorWithDefaultFallback(SrgbaTuple::from((
        0xcd_u8, 0x00_u8, 0x00_u8, 0xff_u8,
    )))
}

/// Text a program printed arrives on the screen, at the columns it was printed at, across lines.
#[test]
fn printed_text_lands_where_the_program_put_it() {
    let grid = pane(b"one\r\ntwo\r\n  three", 12, 3);
    let surface = painted(&grid);
    let lines: Vec<String> = surface
        .screen_chars_to_string()
        .lines()
        .map(|line| line.trim_end().to_owned())
        .collect();
    assert_eq!(lines, ["one", "two", "  three"]);
}

/// The SGR attribute battery survives the whole chain. Each cell is checked on the axis its own
/// sequence set, so a mapping that collapsed everything to "styled" would fail on the first one it
/// dropped rather than pass because something was set.
#[test]
fn the_sgr_battery_survives_the_whole_chain() {
    // bold / dim / italic / single-underline / curly-underline / reverse / strikethrough / blink,
    // each on its own cell, each reset before the next so the axes stay independent.
    let grid = pane(
        b"\x1b[1mB\x1b[0m\x1b[2mD\x1b[0m\x1b[3mI\x1b[0m\x1b[4mU\x1b[0m\
          \x1b[4:3mC\x1b[0m\x1b[7mR\x1b[0m\x1b[9mS\x1b[0m\x1b[5mK\x1b[0m",
        10,
        1,
    );
    let mut surface = painted(&grid);
    let cells = surface.screen_cells();
    let attrs: Vec<_> = (0..8).map(|col| cells[0][col].attrs().clone()).collect();

    assert_eq!(attrs[0].intensity(), Intensity::Bold, "SGR 1");
    assert_eq!(attrs[1].intensity(), Intensity::Half, "SGR 2");
    assert!(attrs[2].italic(), "SGR 3");
    assert_eq!(attrs[3].underline(), Underline::Single, "SGR 4");
    assert_eq!(attrs[4].underline(), Underline::Curly, "SGR 4:3");
    assert!(attrs[5].reverse(), "SGR 7");
    assert!(attrs[6].strikethrough(), "SGR 9");
    // pinion folds SGR 5 and 6 into one flag, so the only blink a client can be handed is the
    // slow one — asserting the VARIANT rather than "is it blinking" is what pins that.
    assert_eq!(attrs[7].blink(), Blink::Slow, "SGR 5");
}

/// Colour crosses as the producer resolved it: an indexed foreground arrives truecolor, and a
/// truecolor stays itself. The second half is what proves the first is not an accident of the
/// palette happening to be the identity.
#[test]
fn colour_arrives_as_the_producer_resolved_it() {
    let grid = pane(b"\x1b[31mR\x1b[0m\x1b[38;2;18;52;86mT", 6, 1);
    let mut surface = painted(&grid);
    let cells = surface.screen_cells();
    assert_eq!(cells[0][0].attrs().foreground(), xterm_red());
    assert_eq!(
        cells[0][1].attrs().foreground(),
        ColorAttribute::TrueColorWithDefaultFallback(SrgbaTuple::from((
            0x12_u8, 0x34_u8, 0x56_u8, 0xff_u8
        ))),
    );
}

/// A background colour spans BOTH columns of a wide cluster. This is the case the trailer exists
/// for: the trailer has no glyph, so if its column took its colour from anywhere but the head, a
/// coloured CJK run would be striped.
#[test]
fn a_wide_clusters_background_spans_both_of_its_columns() {
    let grid = pane("\x1b[41m한\x1b[0m!".as_bytes(), 8, 1);
    let mut surface = painted(&grid);
    let cells = surface.screen_cells();
    let red_bg = ColorAttribute::TrueColorWithDefaultFallback(SrgbaTuple::from((
        0xcd_u8, 0x00_u8, 0x00_u8, 0xff_u8,
    )));
    assert_eq!(cells[0][0].attrs().background(), red_bg, "the head");
    assert_eq!(cells[0][1].attrs().background(), red_bg, "the trailer");
    assert_eq!(cells[0][2].str(), "!", "and the next cell is not displaced");
}

/// A real emulator's wide text keeps the COLUMNS the emulator assigned it — the alignment
/// invariant the painter's width cross-check protects, measured here against the emulator that
/// actually assigns those columns rather than against a cell built to agree with the painter.
///
/// This is the test that would catch a disagreement between `sprag-vt`'s width tables
/// (`unicode-width`) and termwiz's (`widechar_width`), since only one of them decided each side of
/// the comparison.
#[test]
fn wide_text_keeps_the_columns_the_emulator_gave_it() {
    let grid = pane("가나다|".as_bytes(), 12, 1);
    let mut surface = painted(&grid);
    let cells = surface.screen_cells();
    assert_eq!(cells[0][0].str(), "가");
    assert_eq!(cells[0][2].str(), "나");
    assert_eq!(cells[0][4].str(), "다");
    assert_eq!(
        cells[0][6].str(),
        "|",
        "three wide clusters take six columns"
    );
}

/// THE fidelity invariant, stated once and checked over the clusters most likely to break it:
/// **every cell the projection placed at `(col, row)` is on the screen at `(col, row)`.**
///
/// This is the assertion the painter's width cross-check exists to keep true, and it is checked
/// against the projection's OWN column assignment rather than against numbers written here — so it
/// stays honest if `sprag-vt`'s width tables ever move.
///
/// The battery is chosen for disagreement risk between the two independent width implementations
/// in play (`unicode-width` in `sprag-vt`, `widechar_width` in termwiz): a combining sequence, an
/// emoji, a ZWJ family, and two East-Asian AMBIGUOUS-width characters, which are exactly where
/// implementations and Unicode versions diverge. If one of them ever does diverge here, this fails
/// — and it fails naming the character, which is the whole point of driving the real emulator.
#[test]
fn every_projected_cell_lands_at_its_own_column() {
    for text in ["éa", "👍a", "👨‍👩‍👧a", "①a", "√a", "가a"] {
        let grid = pane(text.as_bytes(), 16, 1);
        let mut surface = painted(&grid);
        let painted_row: Vec<String> = surface.screen_cells()[0]
            .iter()
            .map(|cell| cell.str().to_owned())
            .collect();
        for col in 0..grid.cols() {
            let cell = grid
                .cell(col, 0)
                .expect("the projection sized its own grid");
            // A trailer has no glyph of its own on either side of the comparison.
            if cell.width == pinion_core::CellWidth::Trailer {
                continue;
            }
            assert_eq!(
                painted_row[usize::from(col)],
                cell.cluster,
                "{text:?}: the projection put {:?} at column {col}, the screen has {:?}",
                cell.cluster,
                painted_row[usize::from(col)],
            );
        }
    }
}

/// An OSC 8 hyperlink survives as a hyperlink, not as text — including its grouping id, which is
/// what makes two runs of a soft-wrapped link one link.
#[test]
fn an_osc_8_hyperlink_crosses_as_a_hyperlink() {
    let grid = pane(
        b"\x1b]8;id=x1;https://example.com\x1b\\link\x1b]8;;\x1b\\.",
        12,
        1,
    );
    let mut surface = painted(&grid);
    let cells = surface.screen_cells();
    let link = cells[0][0]
        .attrs()
        .hyperlink()
        .cloned()
        .expect("the first cell of the run carries the link");
    assert_eq!(link.uri(), "https://example.com");
    assert_eq!(link.params().get("id").map(String::as_str), Some("x1"));
    // Every cell of the run carries it, and the cell after the close does not.
    assert!(cells[0][3].attrs().hyperlink().is_some(), "the run's last");
    assert!(cells[0][4].attrs().hyperlink().is_none(), "after the close");
}

/// The pane's cursor becomes the terminal's cursor, so the view reads as a terminal rather than a
/// picture of one. `ESC [ 2 ; 5 H` is 1-based; the surface is 0-based.
#[test]
fn the_panes_cursor_becomes_the_terminals() {
    let grid = pane(b"\x1b[2;5H", 10, 3);
    let surface = painted(&grid);
    assert_eq!(surface.cursor_position(), (4, 1));
}

/// A hidden cursor (DECTCEM off) is hidden on the local terminal too — a client that always drew
/// one would put a block in the middle of a full-screen editor that deliberately hid it.
#[test]
fn a_hidden_cursor_stays_hidden() {
    let visible = painted(&pane(b"\x1b[?25h", 4, 1));
    let hidden = painted(&pane(b"\x1b[?25l", 4, 1));
    assert_eq!(
        visible.cursor_visibility(),
        termwiz::surface::CursorVisibility::Visible,
    );
    assert_eq!(
        hidden.cursor_visibility(),
        termwiz::surface::CursorVisibility::Hidden,
    );
}
