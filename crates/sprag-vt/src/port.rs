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

use unicode_width::UnicodeWidthStr;

/// The number of terminal columns a `char` occupies (UAX #11 via
/// [`unicode_width`]): `0` for a zero-width combining mark (merged into the
/// preceding cell, not its own column), `1` narrow, `2` wide (CJK / full-width).
///
/// The single width authority. The emulator's print path
/// ([`Emulator::print_str`](crate::emulator)) and any out-of-band projection
/// overlay that must place not-yet-printed text (the GUI IME preedit's
/// `sprag_grid::overlay_preedit`) both classify a glyph's cell span through here,
/// so they cannot disagree on width — the producer owns the width model and
/// exposes it rather than letting each consumer recompute it.
#[must_use]
pub fn char_columns(ch: char) -> usize {
    UnicodeWidthStr::width(ch.to_string().as_str())
}

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
    /// Per-row soft-wrap continuation flag (the DEC `LINE_WRAPPED` attribute):
    /// `wrapped[r] == true` means row `r`'s logical line CONTINUES onto row
    /// `r + 1`, so a reflow ([`Self::reflowed`]) joins them into one logical line
    /// before re-breaking to a new width. Without it a resize cannot tell a soft
    /// wrap from a hard newline, so it cannot rewrap (the verbatim
    /// [`Self::resized`] fallback leaves a live shell's per-width prompt redraws
    /// stacked up).
    ///
    /// Set TRUE by two producers: (1) the autowrap site (the emulator hit the right
    /// margin); (2) the line editor's resize-redraw `CR LF` continuation
    /// (`Emulator::in_resize_redraw` — a premature break that is semantically a soft
    /// wrap). Cleared when a row is erased or a line feed ends the line OUTSIDE that
    /// redraw. The second producer is deliberate, not a stray writer: a reflowing
    /// terminal must treat the editor's redraw continuation as soft for it to
    /// collapse on widen, and that `CR LF` is context-only-distinguishable from a
    /// hard newline (it lands mid-row), so the emulator owns that one decision.
    wrapped: Vec<bool>,
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
            wrapped: vec![false; rows as usize],
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

    /// Whether row `row` soft-wraps onto the next row (its logical line
    /// continues). `false` out of bounds. See [`Self::reflowed`].
    #[must_use]
    pub fn wrapped(&self, row: u16) -> bool {
        self.wrapped.get(row as usize).copied().unwrap_or(false)
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

    /// Mark (or clear) row `row`'s soft-wrap continuation flag. The VT backend
    /// sets it at the autowrap site and clears it when a row is erased or a hard
    /// line feed ends the line. Out-of-bounds rows are ignored.
    pub(crate) fn set_wrapped(&mut self, row: u16, wrapped: bool) {
        if let Some(slot) = self.wrapped.get_mut(row as usize) {
            *slot = wrapped;
        }
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
            // An erased row no longer continues a logical line.
            self.wrapped[row as usize] = false;
        }
    }

    /// Clear the soft-wrapped CONTINUATION rows of the logical line whose head is
    /// `row` (the rows `row+1..` reached by following the [`Self::wrapped`] chain),
    /// leaving `row` itself untouched. One atomic operation that keeps the
    /// soft-wrap invariant on the `Screen` (the SSOT owner): the chain is measured
    /// to its last row BEFORE any clearing, because [`Self::clear_row`] drops a
    /// row's own wrap flag (clearing as we walk would cut the walk short).
    ///
    /// Used by a line editor's resize redraw ([`crate::emulator`]): when the editor
    /// reprints a wrapped line from its head, the stale tail the prior width left
    /// below — which the reprint may only partly overwrite — must go, or it lingers
    /// as a growing leftover. Caller-bounded to that redraw; a plain erase does not
    /// touch continuation rows.
    pub(crate) fn clear_soft_wrap_continuation(&mut self, row: u16, generation: u64) {
        let mut last = row;
        while last + 1 < self.rows && self.wrapped(last) {
            last += 1;
        }
        for r in (row + 1)..=last {
            self.clear_row(r, generation);
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
            next.wrapped[r as usize] = self.wrapped[r as usize];
        }
        next.cursor = self.cursor;
        next.kind = self.kind;
        // Scrollback is text history, independent of the grid dimensions; carry
        // it across verbatim (lines are not reflowed to the new width).
        next.scrollback = self.scrollback.clone();
        next
    }

    /// A copy reflowed to `cols x rows`: the visible MAIN screen's LOGICAL lines
    /// (physical rows joined by the soft-wrap flag, [`Self::wrapped`]) are
    /// re-broken at the new width, so a resize rewraps cleanly instead of leaving
    /// a live shell's per-width prompt redraws stacked up (the verbatim
    /// [`Self::resized`] bug). The alternate screen and degenerate sizes fall back
    /// to [`Self::resized`] (a fullscreen app owns its own layout). The cursor
    /// tracks its LOGICAL line across the rewrap but anchors to that line's FIRST
    /// physical row (its column preserved), so a live line editor's resize redraw
    /// overwrites in place rather than stacking — see the cursor-anchor note in
    /// Pass 3. Wide clusters never split across the margin; an overflow on a
    /// narrower reflow scrolls the top off into scrollback (text-only), keeping the
    /// cursor visible. `gen` is a fresh damage stamp for every (re-laid-out) row.
    pub(crate) fn reflowed(&self, cols: u16, rows: u16, generation: u64) -> Screen {
        if self.kind != ScreenKind::Main || cols == 0 || rows == 0 {
            return self.resized(cols, rows);
        }
        // Pass 1 — reconstruct logical lines from glyph cells (trailers dropped),
        // joining soft-wrapped rows; trim trailing blanks at a hard line end.
        // Track the cursor's (logical line, glyph offset).
        let mut lines: Vec<Vec<Cell>> = Vec::new();
        let mut cur: Vec<Cell> = Vec::new();
        let (cur_col, cur_row) = (self.cursor.col, self.cursor.row);
        let (mut cursor_line, mut cursor_off, mut cursor_found) = (0usize, 0usize, false);
        for r in 0..self.rows {
            let mut glyphs: Vec<Cell> = Vec::new();
            for c in 0..self.cols {
                if !cursor_found && r == cur_row && c == cur_col {
                    cursor_found = true;
                    cursor_line = lines.len();
                    cursor_off = cur.len() + glyphs.len();
                }
                let cell = self.cell(c, r).cloned().unwrap_or_else(Cell::blank);
                if cell.width == Width::Trailer {
                    continue; // regenerated when the wide head is re-placed
                }
                glyphs.push(cell);
            }
            let wrapped = self.wrapped(r);
            if !wrapped {
                while glyphs
                    .last()
                    .is_some_and(|c| c.width == Width::Narrow && c.cluster == " ")
                {
                    glyphs.pop();
                }
            }
            cur.extend(glyphs);
            if !wrapped {
                if cursor_found && cursor_line == lines.len() {
                    cursor_off = cursor_off.min(cur.len());
                }
                lines.push(std::mem::take(&mut cur));
            }
        }
        if !cur.is_empty() {
            lines.push(cur);
        }
        // Drop trailing EMPTY logical lines (blank padding below the content), so
        // they neither consume rows nor push the content off the top via the
        // bottom-anchor — but never drop the cursor's line or above.
        while lines.len() > cursor_line + 1 && lines.last().is_some_and(Vec::is_empty) {
            lines.pop();
        }
        if lines.is_empty() {
            lines.push(Vec::new());
        }
        if !cursor_found {
            cursor_line = lines.len() - 1;
            cursor_off = lines[cursor_line].len();
        }
        // Pass 2 — re-flow each logical line into the new width (a wide cluster
        // moves whole), recording per-row soft-wrap flags and the cursor's new
        // (col, physical-row).
        let mut phys: Vec<(Vec<Cell>, bool)> = Vec::new();
        let mut cursor_phys: Option<(u16, usize)> = None;
        // The first physical row of the cursor's logical line — the row a line
        // editor's resize redraw rewrites from (see the cursor-anchor note below).
        let mut cursor_line_top: usize = 0;
        for (li, line) in lines.iter().enumerate() {
            if li == cursor_line {
                cursor_line_top = phys.len();
            }
            let mut buf: Vec<Cell> = Vec::new();
            let mut col: u16 = 0;
            for (i, cell) in line.iter().enumerate() {
                if cursor_phys.is_none() && cursor_line == li && cursor_off == i {
                    cursor_phys = Some((col, phys.len()));
                }
                let w: u16 = if cell.width == Width::Wide { 2 } else { 1 };
                if col + w > cols {
                    phys.push((std::mem::take(&mut buf), true)); // soft-wrap break
                    col = 0;
                }
                buf.push(cell.clone());
                if cell.width == Width::Wide {
                    buf.push(Cell::trailer_for(cell));
                }
                col += w;
            }
            if cursor_phys.is_none() && cursor_line == li && cursor_off >= line.len() {
                cursor_phys = Some((col, phys.len()));
            }
            phys.push((buf, false)); // hard end of this logical line
        }
        // Pass 3 — materialize, bottom-anchored: the bottom `rows` physical rows
        // are visible; any overflow scrolls off the top into scrollback.
        let ncols = cols as usize;
        let keep = rows as usize;
        let total = phys.len();
        let start = total.saturating_sub(keep);
        let mut next = Screen::new(cols, rows);
        next.scrollback = self.scrollback.clone();
        for (cells, _) in phys.iter().take(start) {
            let text: String = cells.iter().map(|c| c.cluster.as_str()).collect();
            next.scrollback.push_back(text.trim_end().to_string());
        }
        while next.scrollback.len() > SCROLLBACK_CAP {
            next.scrollback.pop_front();
        }
        for (out_r, (cells, wrapped)) in phys[start..].iter().enumerate() {
            for (c, cell) in cells.iter().take(ncols).enumerate() {
                next.cells[out_r * ncols + c] = cell.clone();
            }
            next.wrapped[out_r] = *wrapped;
            next.generations[out_r] = generation;
        }
        // Anchor the cursor to the FIRST physical row of its logical line, not the
        // physical row its glyph offset lands on. A line editor (bash/readline, zsh)
        // redraws its line on `SIGWINCH` by issuing CR + erase-in-line + reprint and
        // navigating relative to where it believes the cursor is — and after an
        // autowrapping reprint it believes the cursor is back at the line's TOP (it
        // emits no cursor-up). If a reflow leaves the cursor at the rewrapped line's
        // BOTTOM row, that CR lands mid-line and the reprint stacks BELOW the old
        // text, so per-width prompt redraws pile up (the exact accumulation this
        // reflow exists to prevent). Keeping the editor in sync means the cursor must
        // sit on the line's first physical row after a reflow.
        //
        // Column: keep the natural column ONLY when the cursor already sits on that
        // first row (a single-row line, where top == the cursor's own row — so its
        // position is unchanged). When the cursor is pulled UP from a lower wrapped
        // row, its natural column is `offset % width`, which slides as a multi-row
        // line re-breaks at different widths — a live splitter drag would paint the
        // cursor skating left/right across the prompt's first row. Pin it to column 0
        // (the line's start) instead: the editor's CR resets the column on its next
        // redraw anyway, and a still column reads as a stable caret, not a jumping one.
        let (ccol, cphys) = cursor_phys.unwrap_or((0, total.saturating_sub(1)));
        let (cur_col, cur_phys) = if cphys == cursor_line_top {
            (ccol, cphys)
        } else {
            (0, cursor_line_top)
        };
        next.cursor = Cursor {
            col: cur_col.min(cols.saturating_sub(1)),
            row: (cur_phys.saturating_sub(start)).min(keep.saturating_sub(1)) as u16,
            shape: self.cursor.shape,
            visible: self.cursor.visible,
        };
        next.kind = self.kind;
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
        self.cells
            .extend(std::iter::repeat_with(Cell::blank).take(cols));
        // Shift the wrap flags up in lockstep with the rows; the new bottom row
        // is blank (not a continuation). (`rows > 0` is guaranteed above.)
        self.wrapped.remove(0);
        self.wrapped.push(false);
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
        assert_eq!(
            e.screen().scrollback_len(),
            0,
            "alt screen has no scrollback"
        );
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
        assert_eq!(
            e.screen().scrollback_len(),
            0,
            "ED-3 should clear scrollback"
        );
    }
}
