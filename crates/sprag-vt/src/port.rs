//! The VT port: sprag-owned, library-agnostic terminal screen model.
//!
//! These types are the stable seam between the VT backend (currently a
//! termwiz-based emulator in [`crate::emulator`]) and the consumer
//! ([`sprag-grid`]'s pinion projection). Nothing here depends on termwiz,
//! so the VT library choice stays reversible (DESIGN.md §4: VtPort
//! isolates the max-risk VT dependency).
//!
//! The model mirrors pinion's `GridBuffer` data model one-to-one (cells
//! carry fg/bg/attrs/width; the screen carries a cursor, a screen kind,
//! and per-row damage generations) so the projection is a flat mapping
//! rather than a translation (DESIGN.md §3).

use std::collections::VecDeque;

/// Maximum number of scrolled-off lines [`Screen`] retains (FIFO). Bounds
/// memory under unbounded output; the oldest line drops past this.
pub(crate) const SCROLLBACK_CAP: usize = 1000;

/// A 24-bit truecolor value.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct Rgb {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

impl Rgb {
    #[must_use]
    pub const fn new(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b }
    }
}

/// A terminal cell color, mirroring the three SGR color forms and
/// pinion's `TermColor`. `Default` defers to the palette at read time.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Color {
    #[default]
    Default,
    Indexed(u8),
    Rgb(Rgb),
}

/// SGR display attributes — the eight booleans pinion's `CellAttrs` models.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct Attrs {
    pub bold: bool,
    pub dim: bool,
    pub italic: bool,
    pub underline: bool,
    pub blink: bool,
    pub reverse: bool,
    pub hidden: bool,
    pub strikethrough: bool,
}

/// Display-width role of a cell (mirrors pinion's `CellWidth`).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Width {
    /// Single-column cluster.
    #[default]
    Narrow,
    /// Wide-cluster head (occupies this column plus the next).
    Wide,
    /// Continuation column of a `Wide` head (no independent glyph).
    Trailer,
}

/// A single terminal cell.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Cell {
    /// Grapheme cluster. `" "` for a blank cell, `""` for a wide trailer.
    pub cluster: String,
    pub fg: Color,
    pub bg: Color,
    pub attrs: Attrs,
    pub width: Width,
}

impl Default for Cell {
    fn default() -> Self {
        Self {
            cluster: " ".to_string(),
            fg: Color::Default,
            bg: Color::Default,
            attrs: Attrs::default(),
            width: Width::Narrow,
        }
    }
}

impl Cell {
    /// A blank narrow cell with default colors and no attributes.
    #[must_use]
    pub fn blank() -> Self {
        Self::default()
    }

    /// The trailing cell for a wide head: empty cluster, head's colors.
    #[must_use]
    pub fn trailer_for(head: &Cell) -> Self {
        Self {
            cluster: String::new(),
            fg: head.fg,
            bg: head.bg,
            attrs: head.attrs,
            width: Width::Trailer,
        }
    }
}

/// Cursor shape (DECSCUSR), mirroring pinion's `CursorShape`.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum CursorShape {
    #[default]
    Block,
    Bar,
    Underline,
}

/// Cursor position and presentation. Coordinates are buffer-space cells.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Cursor {
    pub col: u16,
    pub row: u16,
    pub shape: CursorShape,
    pub visible: bool,
}

impl Default for Cursor {
    fn default() -> Self {
        Self {
            col: 0,
            row: 0,
            shape: CursorShape::Block,
            visible: true,
        }
    }
}

/// Which screen buffer is active (DECSET 1049), mirroring `ScreenKind`.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum ScreenKind {
    #[default]
    Main,
    Alternate,
}

/// Terminal input modes that change how keys encode to PTY bytes.
///
/// These are set by the child process via escape sequences (parsed by
/// the emulator) and read by the sprag-owned key encoder (encoding is
/// sprag's responsibility — PINION-REQUIREMENTS R2.6). They are not
/// screen-grid state, so they sit alongside the [`Screen`] on the port
/// rather than inside it. The struct grows as more input-affecting
/// modes are modeled (application keypad, modifyOtherKeys).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct InputModes {
    /// DECCKM (DEC private mode 1). When set, cursor and edit keys are
    /// sent with `SS3` (`ESC O`) introducers instead of `CSI`
    /// (`ESC [`); full-screen apps (vim, less) enable it, so the arrow
    /// and Home/End encodings flip on this flag.
    pub application_cursor_keys: bool,
}

/// A queryable terminal screen: a `cols x rows` grid of cells plus the
/// cursor, screen kind, and per-row damage generations.
///
/// This is the authoritative terminal state sprag owns (DESIGN.md §3:
/// the producer owns state; pinion is a projection). A VT backend fills
/// it; the projection reads it.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Screen {
    cols: u16,
    rows: u16,
    /// Row-major, `rows * cols` cells.
    cells: Vec<Cell>,
    cursor: Cursor,
    kind: ScreenKind,
    /// One monotonic damage stamp per row.
    generations: Vec<u64>,
    /// Trailing-trimmed TEXT of rows scrolled off the top of the MAIN screen,
    /// oldest first, bounded by [`SCROLLBACK_CAP`] (FIFO). Text-only (not full
    /// cells) — the consumer is data capture, not rendering; lines are NOT
    /// reflowed on resize. The visible scene projection ignores this.
    scrollback: VecDeque<String>,
}

impl Screen {
    /// A blank `cols x rows` screen, every row at generation 0.
    #[must_use]
    pub fn new(cols: u16, rows: u16) -> Self {
        let count = cols as usize * rows as usize;
        Self {
            cols,
            rows,
            cells: vec![Cell::blank(); count],
            cursor: Cursor::default(),
            kind: ScreenKind::Main,
            generations: vec![0; rows as usize],
            scrollback: VecDeque::new(),
        }
    }

    #[must_use]
    pub const fn cols(&self) -> u16 {
        self.cols
    }

    #[must_use]
    pub const fn rows(&self) -> u16 {
        self.rows
    }

    #[must_use]
    pub const fn cursor(&self) -> Cursor {
        self.cursor
    }

    #[must_use]
    pub const fn screen_kind(&self) -> ScreenKind {
        self.kind
    }

    fn index(&self, col: u16, row: u16) -> Option<usize> {
        if col < self.cols && row < self.rows {
            Some(row as usize * self.cols as usize + col as usize)
        } else {
            None
        }
    }

    /// Read a cell, or `None` if out of bounds.
    #[must_use]
    pub fn cell(&self, col: u16, row: u16) -> Option<&Cell> {
        self.index(col, row).map(|i| &self.cells[i])
    }

    /// The damage generation for a row, or `None` if out of bounds.
    #[must_use]
    pub fn row_generation(&self, row: u16) -> Option<u64> {
        self.generations.get(row as usize).copied()
    }

    /// A row's text: its cells' clusters concatenated, trailing blanks trimmed.
    /// The canonical row-to-text mapping (the capture path and scrollback both
    /// use it, so they never drift). Wide trailers contribute `""`, blanks `" "`.
    #[must_use]
    pub fn row_text(&self, row: u16) -> String {
        let mut line = String::new();
        for col in 0..self.cols {
            if let Some(cell) = self.cell(col, row) {
                line.push_str(&cell.cluster);
            }
        }
        line.trim_end().to_string()
    }

    /// The scrolled-off lines (oldest first) — the MAIN screen's history beyond
    /// the visible grid, for full-output capture.
    pub fn scrollback_rows(&self) -> impl Iterator<Item = &str> {
        self.scrollback.iter().map(String::as_str)
    }

    /// How many scrolled-off lines are retained.
    #[must_use]
    pub fn scrollback_len(&self) -> usize {
        self.scrollback.len()
    }

    /// The pane's full output text: scrolled-off lines (scrollback) then the
    /// visible rows, trailing empty lines stripped, joined by `"\n"`.
    ///
    /// This is the SINGLE definition of "the pane's text" — both the in-process
    /// capture path (`sprag_plugin`) and the RPC `full_text` query read it, so
    /// the system has one notion of screen text (the `Screen` is the single
    /// source; this and the visible-grid projection are two views of it).
    #[must_use]
    pub fn full_text(&self) -> String {
        let mut lines: Vec<String> = self.scrollback.iter().cloned().collect();
        for row in 0..self.rows {
            lines.push(self.row_text(row));
        }
        while lines.last().is_some_and(String::is_empty) {
            lines.pop();
        }
        lines.join("\n")
    }

    // --- mutation surface for VT backends (crate-internal) ---

    pub(crate) fn set_cursor(&mut self, cursor: Cursor) {
        self.cursor = cursor;
    }

    pub(crate) fn set_kind(&mut self, kind: ScreenKind) {
        self.kind = kind;
    }

    /// Write a cell and bump the owning row's damage generation.
    pub(crate) fn set_cell(&mut self, col: u16, row: u16, cell: Cell, generation: u64) {
        if let Some(i) = self.index(col, row) {
            self.cells[i] = cell;
            self.generations[row as usize] = generation;
        }
    }

    /// Clear a row to blanks and bump its damage generation.
    pub(crate) fn clear_row(&mut self, row: u16, generation: u64) {
        if row < self.rows {
            let start = row as usize * self.cols as usize;
            let end = start + self.cols as usize;
            for c in &mut self.cells[start..end] {
                *c = Cell::blank();
            }
            self.generations[row as usize] = generation;
        }
    }

    /// A copy of this screen resized to `cols x rows`, preserving the
    /// overlapping top-left region, the cursor, and the screen kind.
    pub(crate) fn resized(&self, cols: u16, rows: u16) -> Screen {
        let mut next = Screen::new(cols, rows);
        let copy_cols = cols.min(self.cols);
        let copy_rows = rows.min(self.rows);
        for r in 0..copy_rows {
            for c in 0..copy_cols {
                if let (Some(src), Some(dst_i)) = (self.cell(c, r), next.index(c, r)) {
                    next.cells[dst_i] = src.clone();
                }
            }
            next.generations[r as usize] = self.generations[r as usize];
        }
        next.cursor = self.cursor;
        next.kind = self.kind;
        // Scrollback is text history, independent of the grid dimensions; carry
        // it across verbatim (lines are not reflowed to the new width).
        next.scrollback = self.scrollback.clone();
        next
    }

    /// Drop all retained scrollback (the child sent `ESC [ 3 J`, ED-3).
    pub(crate) fn clear_scrollback(&mut self) {
        self.scrollback.clear();
    }

    /// Scroll the whole screen up by one row; the bottom row becomes blank.
    /// All rows are marked damaged at `generation`.
    pub(crate) fn scroll_up(&mut self, generation: u64) {
        if self.rows == 0 {
            return;
        }
        // Retain the evicted top row's text (MAIN screen only — the alternate
        // screen has no scrollback). Captured before the drain, bounded FIFO.
        if self.kind == ScreenKind::Main && self.cols > 0 {
            self.scrollback.push_back(self.row_text(0));
            while self.scrollback.len() > SCROLLBACK_CAP {
                self.scrollback.pop_front();
            }
        }
        let cols = self.cols as usize;
        self.cells.drain(0..cols);
        self.cells.extend(std::iter::repeat_with(Cell::blank).take(cols));
        for g in &mut self.generations {
            *g = generation;
        }
    }
}

/// The VT port: feed PTY bytes, resize, and read the resulting screen.
///
/// One implementation today (the termwiz-based [`crate::emulator::Emulator`]);
/// the trait is the swappable seam (DESIGN.md §4).
pub trait VtPort {
    /// Feed raw PTY output bytes; updates the screen in place.
    fn advance(&mut self, bytes: &[u8]);

    /// Resize the screen to `cols x rows` cells.
    fn resize(&mut self, cols: u16, rows: u16);

    /// The current authoritative screen.
    fn screen(&self) -> &Screen;

    /// The current input modes affecting key→PTY-byte encoding (DECCKM,
    /// …). Read by the sprag-owned key encoder (R2.6).
    fn input_modes(&self) -> InputModes;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::emulator::Emulator;

    /// Drive scrollback through the real path (advance -> line_feed -> scroll_up).
    fn em(cols: u16, rows: u16, bytes: &str) -> Emulator {
        let mut e = Emulator::new(cols, rows);
        e.advance(bytes.as_bytes());
        e
    }

    #[test]
    fn scrollback_captures_evicted_lines_in_order() {
        // A 2-row screen; four lines push the first two into scrollback.
        let e = em(8, 2, "1\r\n2\r\n3\r\n4");
        let sb: Vec<&str> = e.screen().scrollback_rows().collect();
        assert_eq!(sb, ["1", "2"], "scrolled-off lines, oldest first");
        // The visible grid holds the last two.
        assert_eq!(e.screen().row_text(0), "3");
        assert_eq!(e.screen().row_text(1), "4");
    }

    #[test]
    fn scrollback_cap_is_fifo() {
        // On a 1-row screen each newline scrolls; feed past the cap.
        let n = SCROLLBACK_CAP + 100;
        let input: String = (0..n).map(|i| format!("{i}\r\n")).collect();
        let e = em(12, 1, &input);
        assert_eq!(e.screen().scrollback_len(), SCROLLBACK_CAP, "bounded");
        // The oldest 100 lines (0..100) were dropped; 100 is now the oldest.
        assert_eq!(e.screen().scrollback_rows().next(), Some("100"));
    }

    #[test]
    fn alt_screen_does_not_accumulate_or_disturb_scrollback() {
        let mut e = em(8, 2, "a\r\nb\r\nc"); // main scrolls: scrollback ["a"]
        assert_eq!(e.screen().scrollback_rows().collect::<Vec<_>>(), ["a"]);
        // Enter alt and scroll it a lot — must not touch main's scrollback.
        e.advance(b"\x1b[?1049h");
        e.advance(b"p\r\nq\r\nr\r\ns\r\nt");
        assert_eq!(e.screen().scrollback_len(), 0, "alt screen has no scrollback");
        // Exit alt: the parked main screen (and its scrollback) is restored.
        e.advance(b"\x1b[?1049l");
        assert_eq!(e.screen().scrollback_rows().collect::<Vec<_>>(), ["a"]);
    }

    #[test]
    fn resize_preserves_scrollback() {
        let mut e = em(8, 2, "a\r\nb\r\nc"); // scrollback ["a"]
        e.resize(12, 4);
        assert_eq!(e.screen().scrollback_rows().collect::<Vec<_>>(), ["a"]);
    }

    #[test]
    fn wide_cluster_scrolled_off_keeps_full_text() {
        // A wide head + empty trailer must reconstruct to the single cluster.
        let e = em(8, 2, "\u{4e16}\r\n\r\n");
        assert_eq!(e.screen().scrollback_rows().next(), Some("\u{4e16}"));
    }

    #[test]
    fn full_text_joins_scrollback_then_visible_trailing_stripped() {
        // 4 lines on a 2-row screen: "1","2" scroll off, "3","4" visible.
        let e = em(8, 2, "1\r\n2\r\n3\r\n4");
        assert_eq!(e.screen().full_text(), "1\n2\n3\n4");
    }

    #[test]
    fn ed3_clears_scrollback() {
        // Populate scrollback, then ESC[3J drops it.
        let mut e = em(8, 2, "1\r\n2\r\n3");
        assert!(e.screen().scrollback_len() > 0);
        e.advance(b"\x1b[3J");
        assert_eq!(e.screen().scrollback_len(), 0, "ED-3 should clear scrollback");
    }
}
