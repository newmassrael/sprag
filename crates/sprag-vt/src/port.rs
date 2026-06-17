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
        next
    }

    /// Scroll the whole screen up by one row; the bottom row becomes blank.
    /// All rows are marked damaged at `generation`.
    pub(crate) fn scroll_up(&mut self, generation: u64) {
        if self.rows == 0 {
            return;
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
