//! The VT port: sprag-owned, library-agnostic terminal screen model.
//!
//! These types are the stable seam between the VT backend (currently a
//! termwiz-based emulator in [`crate::emulator`]) and the consumer
//! (`sprag-grid`'s pinion projection). Nothing here depends on termwiz,
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

/// A cell row's text: clusters concatenated, trailing blanks trimmed. The ONE
/// row-to-text mapping shared by [`Screen::row_text`] (visible rows) and
/// [`Screen::scrollback_rows`] (scrolled-off rows), so the capture path and the
/// scrollback never drift. Wide trailers contribute `""`, blank cells `" "`.
#[must_use]
fn cells_text(cells: &[Cell]) -> String {
    let mut line = String::new();
    for cell in cells {
        line.push_str(&cell.cluster);
    }
    line.trim_end().to_string()
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

/// A desktop-style ATTENTION notification a child raised out-of-band — an
/// `OSC 9` (iTerm2/xterm `SystemNotification`), an `OSC 777;notify;…` (urxvt),
/// or an `OSC 99` (kitty) — captured so the multiplexer can surface "this pane
/// wants attention" (the tmux bell / cmux "N notifications" analog).
///
/// A DISPLAY signal, never identity — a child sets it freely, exactly like the
/// window [`title`](VtPort::title). Both fields are child-controlled and clamped
/// (see [`crate::emulator`]). `title` is the short heading (`None` for `OSC 9`,
/// which carries only a message); `body` is the message text (which may be empty
/// for a kitty title-only notification).
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct Notification {
    /// The notification's short heading, or `None` when the source carried only a
    /// message body (`OSC 9`).
    pub title: Option<String>,
    /// The notification's message text. May be empty (a kitty title-only chunk).
    pub body: String,
}

/// A shell-integration (OSC 133 / FinalTerm) boundary mark attached to a row: the point at
/// which a shell prompt, a command's output, or a finished command begins. These are the
/// `A`/`C`/`D` semantic markers a shell with integration configured emits, and they are what
/// enables jump-to-prompt navigation and command-boundary extraction (the modern-terminal
/// feature — Ghostty / wezterm / iTerm2; tmux only passes OSC 133 through).
///
/// A mark is attached to the ROW its OSC arrived on (row granularity — the useful grain for
/// jump-to-prompt and for slicing a command's output). It MOVES with that row: through scrolling
/// (a marked row that scrolls off the top carries its mark into the scrollback as a
/// `ScrollbackLine`) and through reflow (which re-attaches it to its logical line's first physical
/// row). At most one mark per row — the marks fall on line boundaries and rarely collide; if two
/// land on one row the later wins (a documented bound).
///
/// `B` (OSC 133 ; B, end-of-prompt / start-of-input) is not its own row mark: at row granularity
/// the user's input sits on the [`Prompt`](PromptMark::Prompt) row, so the command text is the
/// rows from the prompt up to the [`Output`](PromptMark::Output) row, and the output is the rows
/// from there to the [`CommandEnd`](PromptMark::CommandEnd) row.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PromptMark {
    /// `OSC 133 ; A` — a shell prompt starts on this row (the jump-to-prompt target).
    Prompt,
    /// `OSC 133 ; C` — the command was executed; its output starts on this row.
    Output,
    /// `OSC 133 ; D [; exit]` — the command finished on this row, carrying its exit status when
    /// the shell reported one (`None` when it emitted a bare `D`).
    CommandEnd(Option<i32>),
}

/// A pane's shell-integration activity state, DERIVED from its most recent [`PromptMark`] — the
/// cheap "is this shell idle at a prompt, or running a command?" summary a monitor surfaces (the
/// multiplexer / an AI watching a sibling pane). The marks are the source of truth; this is a
/// projection of them, so there is one store, not a latched duplicate that could drift.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum ShellState {
    /// No OSC 133 mark seen — either no shell integration, or nothing has happened yet.
    #[default]
    Unknown,
    /// Idle at a prompt (the last mark was a prompt start or a finished command).
    AtPrompt,
    /// A command is executing — its output is flowing (the last mark was an output start with no
    /// finish yet).
    Running,
}

impl ShellState {
    /// The wire / display token for a KNOWN state (`"at_prompt"` or `"running"`), or `None` for
    /// [`Unknown`](ShellState::Unknown). The single source of the wire vocabulary — a serializer
    /// omits the key when this is `None`, so a pane without shell integration keeps the pre-OSC133
    /// wire shape (additive).
    #[must_use]
    pub fn wire_str(self) -> Option<&'static str> {
        match self {
            Self::Unknown => None,
            Self::AtPrompt => Some("at_prompt"),
            Self::Running => Some("running"),
        }
    }
}

/// One scrolled-off line: its STYLED cells plus any shell-integration [`PromptMark`] the row
/// carried. Bundling the mark WITH its cells (rather than a parallel deque) makes the two
/// impossible to desync as lines are pushed, popped at the [`SCROLLBACK_CAP`], reflowed, or
/// cloned on resize — the single-source-of-truth shape for scrollback history.
#[derive(Clone, PartialEq, Eq, Debug)]
pub(crate) struct ScrollbackLine {
    pub(crate) cells: Vec<Cell>,
    pub(crate) mark: Option<PromptMark>,
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
    /// Rows scrolled off the top of the MAIN screen, oldest first, bounded by
    /// [`SCROLLBACK_CAP`] (FIFO). Each is the row's STYLED cells (fg/bg/attrs/
    /// width preserved), trailing blanks trimmed — so scrolled-back history paints
    /// with its original colors, not flattened to plain text. The text capture
    /// path derives strings from these cells ([`Screen::scrollback_rows`] /
    /// [`Screen::full_text`]), so there is one source; the grid projection reads
    /// the cells ([`Screen::scrollback_cells`]). Lines are reflowed on resize
    /// (the reflow rejoins/rewraps these cells). Each line also carries any
    /// shell-integration [`PromptMark`] its row held (bundled in [`ScrollbackLine`] so the mark
    /// cannot desync from its cells), so a prompt that scrolls into history stays a jump target.
    scrollback: VecDeque<ScrollbackLine>,
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
    /// Per-row shell-integration boundary mark (OSC 133 `A`/`C`/`D`), `None` on an unmarked row.
    /// Parallel to the visible rows exactly like [`Self::generations`] / [`Self::wrapped`]: the
    /// emulator sets a row's mark when the child emits the OSC, and it moves in lockstep with the
    /// row through scrolling ([`Self::scroll_region_up`] carries an evicted mark into the
    /// scrollback), erasing ([`Self::clear_row`] drops it), and reflow ([`Self::reflowed`]
    /// re-attaches it to the logical line's first physical row). See [`PromptMark`].
    marks: Vec<Option<PromptMark>>,
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
            marks: vec![None; rows as usize],
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
    /// continues). `false` out of bounds. See `Self::reflowed`.
    #[must_use]
    pub fn wrapped(&self, row: u16) -> bool {
        self.wrapped.get(row as usize).copied().unwrap_or(false)
    }

    /// A row's cells (`0..cols`, oldest-left), cloned. The row-to-cells mapping
    /// the scrollback push captures so scrolled-off history keeps its styling.
    /// Internal (`pub(crate)`): the cross-crate grid reads `scrollback_cells`, not
    /// live rows; only `row_text` + the region-scroll scrollback capture use this.
    #[must_use]
    pub(crate) fn row_cells(&self, row: u16) -> Vec<Cell> {
        (0..self.cols)
            .filter_map(|col| self.cell(col, row).cloned())
            .collect()
    }

    /// A row's text: its cells' clusters concatenated, trailing blanks trimmed.
    /// Delegates to `cells_text` — the ONE row-to-text mapping the visible
    /// capture path and the scrollback (cell-derived) both use, so they never
    /// drift. Wide trailers contribute `""`, blanks `" "`.
    #[must_use]
    pub fn row_text(&self, row: u16) -> String {
        cells_text(&self.row_cells(row))
    }

    /// The scrolled-off lines as TEXT (oldest first) — the MAIN screen's history
    /// beyond the visible grid, for full-output capture. Derived from the stored
    /// styled cells via `cells_text` (the capture path keeps one notion of text).
    pub fn scrollback_rows(&self) -> impl Iterator<Item = String> + '_ {
        self.scrollback.iter().map(|line| cells_text(&line.cells))
    }

    /// The scrolled-off lines as STYLED CELLS (oldest first) — the rendering view
    /// the grid projection reads so scrollback paints with its original fg/bg/attrs.
    pub fn scrollback_cells(&self) -> impl Iterator<Item = &[Cell]> + '_ {
        self.scrollback.iter().map(|line| line.cells.as_slice())
    }

    /// The shell-integration [`PromptMark`] of visible row `row`, or `None` (unmarked / out of
    /// bounds). The jump-to-prompt / command-boundary reader — pairs with [`Self::scrollback_mark`]
    /// for the history rows.
    #[must_use]
    pub fn mark(&self, row: u16) -> Option<PromptMark> {
        self.marks.get(row as usize).copied().flatten()
    }

    /// The shell-integration [`PromptMark`] of scrolled-off line `index` (0 = oldest), or `None`.
    #[must_use]
    pub fn scrollback_mark(&self, index: usize) -> Option<PromptMark> {
        self.scrollback.get(index).and_then(|line| line.mark)
    }

    /// The MOST RECENT shell-integration mark in stream order (visible rows scanned bottom-up,
    /// then scrollback newest-first), or `None` if the child has emitted none. The basis of
    /// [`Self::shell_state`]. Bounded: it early-exits at the first mark found.
    fn last_mark(&self) -> Option<PromptMark> {
        (0..self.rows)
            .rev()
            .find_map(|r| self.marks[r as usize])
            .or_else(|| self.scrollback.iter().rev().find_map(|line| line.mark))
    }

    /// The pane's shell-integration [`ShellState`], DERIVED from its most recent mark: an output
    /// start with no finish yet means a command is Running; a prompt start or a finished command
    /// means idle AtPrompt; no marks means Unknown.
    #[must_use]
    pub fn shell_state(&self) -> ShellState {
        match self.last_mark() {
            Some(PromptMark::Output) => ShellState::Running,
            Some(PromptMark::Prompt | PromptMark::CommandEnd(_)) => ShellState::AtPrompt,
            None => ShellState::Unknown,
        }
    }

    /// The exit status of the LAST finished command (the most recent [`PromptMark::CommandEnd`]
    /// that carried one), or `None` when no command has finished with a reported status. Scans
    /// visible rows bottom-up then scrollback newest-first for the boundary. A bare `OSC 133 ; D`
    /// (finished, no status) also yields `None` — pair with [`Self::shell_state`] to tell "no
    /// command ran" from "ran, status unreported".
    #[must_use]
    pub fn last_exit_status(&self) -> Option<i32> {
        (0..self.rows)
            .rev()
            .find_map(|r| match self.marks[r as usize] {
                Some(PromptMark::CommandEnd(status)) => Some(status),
                _ => None,
            })
            .or_else(|| {
                self.scrollback
                    .iter()
                    .rev()
                    .find_map(|line| match line.mark {
                        Some(PromptMark::CommandEnd(status)) => Some(status),
                        _ => None,
                    })
            })
            .flatten()
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
        let mut lines: Vec<String> = self.scrollback_rows().collect();
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

    /// Attach (or clear) a shell-integration [`PromptMark`] on row `row`. The emulator sets it
    /// when the child emits `OSC 133 ; A/C/D` on that row; it then travels with the row through
    /// scrolling and reflow. Out-of-bounds rows are ignored.
    pub(crate) fn set_mark(&mut self, row: u16, mark: Option<PromptMark>) {
        if let Some(slot) = self.marks.get_mut(row as usize) {
            *slot = mark;
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
            // An erased row no longer continues a logical line, and its shell-integration mark
            // (if any) goes with the content that was cleared.
            self.wrapped[row as usize] = false;
            self.marks[row as usize] = None;
        }
    }

    /// Insert `n` blank cells at `(col, row)`, shifting the cells from `col` rightward by `n`
    /// (ICH — INSERT CHARACTER). Cells pushed past the right margin fall off; the opened gap
    /// `[col, col+n)` becomes blank. Row-local (no scroll region interaction), so it is correct
    /// regardless of any top/bottom margins. Bumps the row's damage generation; a shift breaks the
    /// row's soft-wrap continuation (its tail changed), so the wrap flag is cleared.
    pub(crate) fn insert_cells(&mut self, col: u16, row: u16, n: u16, generation: u64) {
        if row >= self.rows || col >= self.cols || n == 0 {
            return;
        }
        let base = row as usize * self.cols as usize;
        let col = col as usize;
        let cols = self.cols as usize;
        let n = (n as usize).min(cols - col);
        // Shift right: move [col, cols-n) to [col+n, cols), walking from the right so a source is
        // read before it is overwritten.
        for dst in (col + n..cols).rev() {
            self.cells[base + dst] = self.cells[base + dst - n].clone();
        }
        for cell in &mut self.cells[base + col..base + col + n] {
            *cell = Cell::blank();
        }
        self.generations[row as usize] = generation;
        self.wrapped[row as usize] = false;
    }

    /// Delete `n` cells at `(col, row)`, shifting the cells from `col+n` leftward to `col` and
    /// blanking the `n` cells vacated at the right margin (DCH — DELETE CHARACTER). Row-local, the
    /// inverse of [`Self::insert_cells`]. Bumps the row's generation and clears its wrap flag.
    pub(crate) fn delete_cells(&mut self, col: u16, row: u16, n: u16, generation: u64) {
        if row >= self.rows || col >= self.cols || n == 0 {
            return;
        }
        let base = row as usize * self.cols as usize;
        let col = col as usize;
        let cols = self.cols as usize;
        let n = (n as usize).min(cols - col);
        // Shift left: move [col+n, cols) to [col, cols-n), walking from the left.
        for dst in col..cols - n {
            self.cells[base + dst] = self.cells[base + dst + n].clone();
        }
        for cell in &mut self.cells[base + cols - n..base + cols] {
            *cell = Cell::blank();
        }
        self.generations[row as usize] = generation;
        self.wrapped[row as usize] = false;
    }

    /// Blank `n` cells at `(col, row)` in place (ECH — ERASE CHARACTER): a bounded erase that,
    /// unlike [`Self::delete_cells`], shifts nothing — the cells right of the erased run stay put.
    /// Row-local; bumps the row's generation.
    pub(crate) fn erase_cells(&mut self, col: u16, row: u16, n: u16, generation: u64) {
        if row >= self.rows || col >= self.cols || n == 0 {
            return;
        }
        let base = row as usize * self.cols as usize;
        let start = col as usize;
        let end = (start + n as usize).min(self.cols as usize);
        for cell in &mut self.cells[base + start..base + end] {
            *cell = Cell::blank();
        }
        self.generations[row as usize] = generation;
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
            next.marks[r as usize] = self.marks[r as usize];
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
    /// narrower reflow scrolls the top off into scrollback (as styled cells), keeping
    /// the cursor visible. `gen` is a fresh damage stamp for every (re-laid-out) row.
    pub(crate) fn reflowed(&self, cols: u16, rows: u16, generation: u64) -> Screen {
        if self.kind != ScreenKind::Main || cols == 0 || rows == 0 {
            return self.resized(cols, rows);
        }
        // Pass 1 — reconstruct logical lines from glyph cells (trailers dropped),
        // joining soft-wrapped rows; trim trailing blanks at a hard line end.
        // Track the cursor's (logical line, glyph offset).
        let mut lines: Vec<Vec<Cell>> = Vec::new();
        // Parallel to `lines`: each logical line's shell-integration mark (its FIRST physical
        // row's), so the mark survives the rewrap by re-attaching to the re-broken line's head.
        let mut line_marks: Vec<Option<PromptMark>> = Vec::new();
        let mut cur: Vec<Cell> = Vec::new();
        let mut cur_mark: Option<PromptMark> = None;
        let (cur_col, cur_row) = (self.cursor.col, self.cursor.row);
        let (mut cursor_line, mut cursor_off, mut cursor_found) = (0usize, 0usize, false);
        for r in 0..self.rows {
            // A logical line's mark is its FIRST physical row's mark; when `cur` is empty this
            // row begins a new logical line, so capture its mark (continuation rows keep it).
            if cur.is_empty() {
                cur_mark = self.marks[r as usize];
            }
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
                line_marks.push(cur_mark);
            }
        }
        if !cur.is_empty() {
            lines.push(cur);
            line_marks.push(cur_mark);
        }
        // Drop trailing EMPTY logical lines (blank padding below the content), so
        // they neither consume rows nor push the content off the top via the
        // bottom-anchor — but never drop the cursor's line or above. `line_marks`
        // is popped in lockstep so it stays parallel to `lines`.
        while lines.len() > cursor_line + 1 && lines.last().is_some_and(Vec::is_empty) {
            lines.pop();
            line_marks.pop();
        }
        if lines.is_empty() {
            lines.push(Vec::new());
            line_marks.push(None);
        }
        if !cursor_found {
            cursor_line = lines.len() - 1;
            cursor_off = lines[cursor_line].len();
        }
        // Pass 2 — re-flow each logical line into the new width (a wide cluster
        // moves whole), recording per-row soft-wrap flags and the cursor's new
        // (col, physical-row).
        // Each entry: (cells, soft-wrapped, mark). The mark rides only the FIRST physical row of
        // a logical line (its head) — where a prompt / output boundary sits.
        let mut phys: Vec<(Vec<Cell>, bool, Option<PromptMark>)> = Vec::new();
        let mut cursor_phys: Option<(u16, usize)> = None;
        // The first physical row of the cursor's logical line — the row a line
        // editor's resize redraw rewrites from (see the cursor-anchor note below).
        let mut cursor_line_top: usize = 0;
        for (li, line) in lines.iter().enumerate() {
            let line_top = phys.len(); // the first physical row this logical line will occupy
            if li == cursor_line {
                cursor_line_top = line_top;
            }
            let mut buf: Vec<Cell> = Vec::new();
            let mut col: u16 = 0;
            for (i, cell) in line.iter().enumerate() {
                if cursor_phys.is_none() && cursor_line == li && cursor_off == i {
                    cursor_phys = Some((col, phys.len()));
                }
                let w: u16 = if cell.width == Width::Wide { 2 } else { 1 };
                if col + w > cols {
                    phys.push((std::mem::take(&mut buf), true, None)); // soft-wrap break
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
            phys.push((buf, false, None)); // hard end of this logical line
            // Re-attach the logical line's mark to its head physical row (always pushed above).
            phys[line_top].2 = line_marks[li];
        }
        // Pass 3 — materialize, bottom-anchored: the bottom `rows` physical rows
        // are visible; any overflow scrolls off the top into scrollback.
        let ncols = cols as usize;
        let keep = rows as usize;
        let total = phys.len();
        let start = total.saturating_sub(keep);
        let mut next = Screen::new(cols, rows);
        next.scrollback = self.scrollback.clone();
        for (cells, _, mark) in phys.iter().take(start) {
            // Keep the styled cells (fg/bg/attrs) — scrollback paints in color — and the row's
            // mark, so a prompt rewrapped into overflow stays a jump target.
            next.scrollback.push_back(ScrollbackLine {
                cells: cells.clone(),
                mark: *mark,
            });
        }
        while next.scrollback.len() > SCROLLBACK_CAP {
            next.scrollback.pop_front();
        }
        for (out_r, (cells, wrapped, mark)) in phys[start..].iter().enumerate() {
            for (c, cell) in cells.iter().take(ncols).enumerate() {
                next.cells[out_r * ncols + c] = cell.clone();
            }
            next.wrapped[out_r] = *wrapped;
            next.marks[out_r] = *mark;
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

    /// Scroll rows `[top, bottom]` (inclusive) UP by `n`: the `n` rows leaving the top of
    /// the region are discarded (or retained as scrollback, see below) and the `n` rows
    /// vacated at the bottom become blank. Rows above `top` and below `bottom` are
    /// untouched. This is the scroll-region primitive behind IND / a line feed at the
    /// bottom margin, SU (`CSI S`), and DL (`CSI M`) — see [`crate::emulator`]. With the
    /// default full-screen region (`top == 0`, `bottom == rows - 1`, `n == 1`) it is the
    /// ordinary "output flows off the top" scroll.
    ///
    /// The rows leaving the top are pushed to the bounded scrollback FIFO — as STYLED
    /// cells, so history paints in its original colors — only when `to_scrollback` is set
    /// AND the region is anchored at the screen top (`top == 0`) on the MAIN screen. That
    /// is history genuinely leaving the top of the screen. `to_scrollback` is `true` for
    /// output-flow scrolls (a line feed at the bottom margin, SU) and `false` for the DL
    /// edit, which REMOVES lines rather than scrolling output away — so a DL at row 0 does
    /// not pollute the scrollback. A mid-screen region (`top > 0`) never reaches the
    /// scrollback regardless (those lines are interior, not off the top). Every row the op
    /// moves or blanks is damaged at `generation`.
    ///
    /// Soft-wrap continuation flags ([`Self::wrapped`]) move in lockstep with the rows;
    /// blanked rows drop their flag. A logical line soft-wrapped ACROSS a region boundary
    /// is a documented bound: scroll regions and reflow do not compose cleanly, and
    /// region-using apps position explicitly rather than relying on autowrap.
    pub(crate) fn scroll_region_up(
        &mut self,
        top: u16,
        bottom: u16,
        n: u16,
        to_scrollback: bool,
        generation: u64,
    ) {
        if self.rows == 0 || self.cols == 0 {
            return;
        }
        let bottom = bottom.min(self.rows - 1);
        if top > bottom {
            return;
        }
        let height = bottom - top + 1;
        let n = n.min(height); // scrolling by >= the region height blanks it whole
        if n == 0 {
            return;
        }
        // Retain the rows leaving the top (`[top, top+n)`) as history, oldest first, only
        // for an output-flow scroll of a top-anchored region on the main screen.
        if to_scrollback && top == 0 && self.kind == ScreenKind::Main {
            for r in 0..n {
                // Carry the row's shell-integration mark into history WITH its cells, so a prompt
                // that scrolls off the top stays a jump target ([`ScrollbackLine`]).
                self.scrollback.push_back(ScrollbackLine {
                    cells: self.row_cells(r),
                    mark: self.marks[r as usize],
                });
            }
            while self.scrollback.len() > SCROLLBACK_CAP {
                self.scrollback.pop_front();
            }
        }
        let cols = self.cols as usize;
        let shift = height - n; // rows that survive and move up by `n`
        // Shift up: move each surviving row `n` positions toward the top. Walk top->bottom
        // so a source row is read before a later iteration overwrites it.
        for i in 0..shift {
            let (dst, src) = ((top + i) as usize, (top + i + n) as usize);
            for c in 0..cols {
                self.cells[dst * cols + c] = self.cells[src * cols + c].clone();
            }
            self.wrapped[dst] = self.wrapped[src];
            self.marks[dst] = self.marks[src];
            self.generations[dst] = generation;
        }
        // Blank the `n` rows vacated at the bottom of the region.
        for i in 0..n {
            self.clear_row(top + shift + i, generation);
        }
    }

    /// Scroll rows `[top, bottom]` (inclusive) DOWN by `n`: the `n` rows leaving the bottom
    /// of the region are discarded and the `n` rows vacated at the top become blank. Rows
    /// above `top` and below `bottom` are untouched. The mirror of [`Self::scroll_region_up`]
    /// behind RI / a reverse index at the top margin, SD (`CSI T`), and IL (`CSI L`). A
    /// down scroll never reaches the scrollback — it discards the bottom, not the top.
    /// Soft-wrap flags move with the rows; blanked top rows drop theirs. Every moved or
    /// blanked row is damaged at `generation`.
    pub(crate) fn scroll_region_down(&mut self, top: u16, bottom: u16, n: u16, generation: u64) {
        if self.rows == 0 || self.cols == 0 {
            return;
        }
        let bottom = bottom.min(self.rows - 1);
        if top > bottom {
            return;
        }
        let height = bottom - top + 1;
        let n = n.min(height);
        if n == 0 {
            return;
        }
        let cols = self.cols as usize;
        let shift = height - n; // rows that survive and move down by `n`
        // Shift down: move each surviving row `n` positions toward the bottom. Walk
        // bottom->top so a source row is read before a later iteration overwrites it.
        for i in 0..shift {
            let (dst, src) = ((bottom - i) as usize, (bottom - n - i) as usize);
            for c in 0..cols {
                self.cells[dst * cols + c] = self.cells[src * cols + c].clone();
            }
            self.wrapped[dst] = self.wrapped[src];
            self.marks[dst] = self.marks[src];
            self.generations[dst] = generation;
        }
        // Blank the `n` rows vacated at the top of the region.
        for i in 0..n {
            self.clear_row(top + i, generation);
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

    /// The child's self-reported window TITLE (`OSC 0` / `OSC 2`), or `None` if it has
    /// never set one. This is LIVE state — a shell's `PROMPT_COMMAND`, vim, ssh or a
    /// nested tmux rewrite it continuously — and is distinct from the pane's spawn
    /// COMMAND LABEL (which names what was launched and never changes). A display
    /// surface prefers this and falls back to a stable name; pane IDENTITY (tags,
    /// panel ids) never derives from it, since a child controls it freely.
    fn title(&self) -> Option<&str>;

    /// The MOST RECENT attention [`Notification`] the child raised (`OSC 9` / `OSC
    /// 777;notify` / `OSC 99`), or `None` if it never raised one. LATCHED (last
    /// value wins) like [`title`](Self::title), so a consumer that wants to detect a
    /// NEW one pairs it with [`notification_seq`](Self::notification_seq) rather than
    /// re-reading the payload.
    fn notification(&self) -> Option<&Notification>;

    /// A monotonic counter bumped once per attention notification raised — `0` before
    /// the first. A consumer that remembers the last value it saw learns a NEW
    /// notification arrived when this grows (the payload alone cannot distinguish a
    /// re-raise of the same text). The multiplexer's "unseen attention" badge is this
    /// minus the last value a viewer acknowledged.
    fn notification_seq(&self) -> u64;

    /// A monotonic counter bumped once per BELL (`\a`) the child rings — `0` before the first.
    /// A bell is the tmux `monitor-bell` signal: a text-less "pay attention" ping, kept SEPARATE
    /// from [`notification_seq`](Self::notification_seq) (a bell is not a desktop toast — it
    /// carries no text) so the two attention sources stay individually addressable, exactly as
    /// tmux keeps its bell flag distinct from activity. A viewer's "unseen attention" combines
    /// both counters (each is monotonic, so their sum is too); a consumer that wants to
    /// distinguish a bell from a notification reads them apart. Only a BARE bell counts — the
    /// `\a` that terminates an OSC string is consumed by the parser as part of that OSC.
    fn bell_seq(&self) -> u64;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::emulator::Emulator;

    /// Drive scrollback through the real path (advance -> line_feed -> scroll_region_up).
    fn em(cols: u16, rows: u16, bytes: &str) -> Emulator {
        let mut e = Emulator::new(cols, rows);
        e.advance(bytes.as_bytes());
        e
    }

    #[test]
    fn scrollback_captures_evicted_lines_in_order() {
        // A 2-row screen; four lines push the first two into scrollback.
        let e = em(8, 2, "1\r\n2\r\n3\r\n4");
        let sb: Vec<String> = e.screen().scrollback_rows().collect();
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
        assert_eq!(e.screen().scrollback_rows().next().as_deref(), Some("100"));
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
        assert_eq!(
            e.screen().scrollback_rows().next().as_deref(),
            Some("\u{4e16}")
        );
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
