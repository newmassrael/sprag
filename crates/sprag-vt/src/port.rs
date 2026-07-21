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
use std::sync::Arc;

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

/// Maximum number of distinct inline [`Image`]s a [`Screen`] retains (FIFO). Bounds memory
/// against a child that transmits many distinct image ids without clearing; past this the
/// oldest is dropped. A screenful of images is a handful, so this is generous.
pub(crate) const IMAGE_CAP: usize = 256;

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

/// The style of a cell's underline — the ECMA-48 SGR `4:x` vocabulary
/// (`4:0`–`4:5` / `21`). Mirrors termwiz's `Underline` and pinion's
/// `UnderlineStyle` one-for-one (same six variants) so the SGR parse and
/// the grid projection are both lossless. A single on/off bool cannot tell
/// an editor's red *curly* LSP error from a blue *dotted* spellcheck; this
/// axis keeps them distinct all the way to the renderer. The underline
/// *colour* (SGR 58 / 59) is the orthogonal [`Cell::underline_color`] axis.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum UnderlineStyle {
    /// SGR 24 / 4:0 — no underline (the default).
    #[default]
    None,
    /// SGR 4 / 4:1 — a single straight rule.
    Single,
    /// SGR 21 / 4:2 — a double straight rule.
    Double,
    /// SGR 4:3 — an undercurl (the squiggle under a diagnostic).
    Curly,
    /// SGR 4:4 — a dotted rule.
    Dotted,
    /// SGR 4:5 — a dashed rule.
    Dashed,
}

impl UnderlineStyle {
    /// `true` for any drawn underline — every variant but [`Self::None`].
    #[must_use]
    pub const fn is_on(self) -> bool {
        !matches!(self, Self::None)
    }
}

/// SGR display attributes pinion's `CellAttrs` models: seven booleans plus
/// the [`UnderlineStyle`] axis (SGR 4:x). The underline *colour* (SGR 58 /
/// 59) is NOT here — it is a third colour channel and lives on
/// [`Cell::underline_color`], a peer of `fg` / `bg`, exactly as the SGR
/// grammar (`58:…` mirrors `38:…` / `48:…`) and pinion's `TermCell` place it.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct Attrs {
    pub bold: bool,
    pub dim: bool,
    pub italic: bool,
    pub underline: UnderlineStyle,
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

/// An OSC-8 hyperlink target a cell can carry: the `uri` the link opens and
/// its optional grouping `id`. sprag-owned and library-agnostic (mirrors
/// pinion's `Hyperlink` and termwiz's, but depends on neither — exactly as
/// [`UnderlineStyle`] mirrors pinion's `UnderlineStyle`).
///
/// A cell references its link by an [`Arc`] shared handle rather than owning
/// the URI: the emulator's OSC-8 pen holds one `Arc` per active link, and
/// every cell printed under it clones that `Arc` (a refcount bump, never the
/// URI string). So a link spanning many cells — including its wrap
/// continuations — stores its URI exactly once, and the cells are recognisable
/// as one link by `Arc` pointer identity. The `Arc` rides physically with the
/// cell, so scroll / scrollback / reflow carry the link for free (no
/// separate interning table to keep in sync, unlike an OSC-133 mark).
///
/// The `id` (`Some`) ties non-adjacent runs that share it into one logical
/// link across a wrap or a repeat; `None` is an anonymous link grouped only
/// within its own contiguous run (so two anonymous links to the same URI are
/// distinct runs — distinct `Arc`s).
#[derive(Clone, PartialEq, Eq, Hash, Debug, Default)]
pub struct Hyperlink {
    /// The URI the link opens (`https://…`, `file://…`, `mailto:…`). Opaque
    /// to the emulator: a consumer decides how to open it (R-69.3 activation).
    pub uri: String,
    /// The OSC-8 `id=` grouping key, or `None` for an anonymous link.
    pub id: Option<String>,
}

/// A single terminal cell.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Cell {
    /// Grapheme cluster. `" "` for a blank cell, `""` for a wide trailer.
    pub cluster: String,
    pub fg: Color,
    pub bg: Color,
    /// SGR 58 / 59 underline colour — a third colour channel, peer of
    /// [`Self::fg`] / [`Self::bg`]. `None` is the SGR-59 default: the
    /// underline (when [`Attrs::underline`] is on) draws in `fg`.
    pub underline_color: Option<Color>,
    pub attrs: Attrs,
    /// The OSC-8 hyperlink this cell belongs to ([`Hyperlink`]), or `None` for
    /// a plain cell. A shared [`Arc`] handle: all cells printed under one
    /// `\e]8;…` pen (a link and its wrap continuations) share the same `Arc`,
    /// so the projection groups them into one link by pointer identity without
    /// cloning the URI per cell. Rides with the cell through scroll / scrollback
    /// / reflow — a link in history keeps its target.
    pub hyperlink: Option<Arc<Hyperlink>>,
    pub width: Width,
}

impl Default for Cell {
    fn default() -> Self {
        Self {
            cluster: " ".to_string(),
            fg: Color::Default,
            bg: Color::Default,
            underline_color: None,
            attrs: Attrs::default(),
            hyperlink: None,
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
            underline_color: head.underline_color,
            attrs: head.attrs,
            // The continuation column of a wide link glyph belongs to the same
            // OSC-8 link, so the whole glyph is one hover / activation target.
            hyperlink: head.hyperlink.clone(),
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
    /// The active Kitty keyboard protocol enhancement flags (`CSI > flags u` and friends).
    /// The sprag-owned key encoder reads these to decide how a key event serializes to the PTY —
    /// unambiguous `CSI u` codes when [`disambiguate`](KittyKeyboardFlags::disambiguate) is on,
    /// legacy bytes otherwise. Empty by default (the legacy encoding). See [`KittyKeyboardFlags`].
    pub kitty_keyboard: KittyKeyboardFlags,
}

/// The Kitty keyboard protocol progressive-enhancement flags currently active — the bitmask a
/// child negotiates via `CSI > flags u` (push) / `CSI = flags ; mode u` (set) / `CSI < n u` (pop),
/// read by the sprag-owned key encoder to serialize key events. The bits mirror the protocol wire
/// values. sprag only advertises + honors the flags it can encode TRUTHFULLY (currently
/// [`DISAMBIGUATE`](Self::DISAMBIGUATE)); a child that requests an unsupported bit sees it dropped
/// at negotiation time (a `CSI ? u` query reports back only the honored subset), so the terminal
/// never claims a capability it does not deliver.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct KittyKeyboardFlags(u8);

impl KittyKeyboardFlags {
    /// `0b1` — *Disambiguate escape codes*: report Esc, and any key held with Ctrl / Alt / Super,
    /// as unambiguous `CSI unicode ; modifiers u` codes instead of the colliding legacy bytes (so
    /// e.g. `Ctrl+i` is distinct from `Tab`, and a lone `Esc` from an escape-sequence prefix).
    pub const DISAMBIGUATE: u8 = 0b1;
    // The higher flags (report event types 0b10, alternate keys 0b100, report all keys as escape
    // codes 0b1000, report associated text 0b10000) are NOT yet honored — negotiating them needs
    // key-release + text plumbing the display client does not yet supply, so they are masked off.

    /// The flags from their raw wire bits.
    #[must_use]
    pub const fn from_bits(bits: u8) -> Self {
        Self(bits)
    }

    /// The raw wire bits (what a `CSI ? flags u` query reports).
    #[must_use]
    pub const fn bits(self) -> u8 {
        self.0
    }

    /// Whether no enhancement is active (the legacy encoding applies).
    #[must_use]
    pub const fn is_empty(self) -> bool {
        self.0 == 0
    }

    /// Whether *Disambiguate escape codes* ([`DISAMBIGUATE`](Self::DISAMBIGUATE)) is active.
    #[must_use]
    pub const fn disambiguate(self) -> bool {
        self.0 & Self::DISAMBIGUATE != 0
    }
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

/// Which system selection an OSC 52 clipboard operation addresses. A windowing system
/// distinguishes two, and sprag models both: the CLIPBOARD (the explicit Ctrl-C / Ctrl-V
/// buffer, OSC 52 `c`) and the PRIMARY selection (X11 select-to-copy / middle-click paste,
/// OSC 52 `p`). The OSC 52 X cut buffers (`0`-`9`) have no windowing-system analog and are
/// not modeled; the "configured selection" `s` and the empty-`Pc` default fold onto the
/// clipboard (the common intent — see [`crate::emulator`]).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ClipboardTarget {
    /// The system clipboard (OSC 52 `c`).
    Clipboard,
    /// The PRIMARY selection (OSC 52 `p`).
    Primary,
}

impl ClipboardTarget {
    /// The OSC 52 selection character (`c` / `p`). A read reply echoes the requested selection
    /// so the asking app matches the response to its query.
    #[must_use]
    pub fn osc_char(self) -> char {
        match self {
            ClipboardTarget::Clipboard => 'c',
            ClipboardTarget::Primary => 'p',
        }
    }
}

/// The set of selections a single OSC 52 WRITE addresses. One `OSC 52 ; cp ; …` sets BOTH the
/// clipboard and the primary selection, so a write is not reducible to one [`ClipboardTarget`];
/// a consumer applies the write text to every selection this marks. A set that names neither
/// (an X-cut-buffer-only request sprag does not model) is [`empty`](Self::is_empty) and the
/// write is dropped.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct ClipboardTargets {
    /// The write addresses the system clipboard (`c`).
    pub clipboard: bool,
    /// The write addresses the PRIMARY selection (`p`).
    pub primary: bool,
}

impl ClipboardTargets {
    /// Whether this addresses no selection sprag models — such a write is a no-op.
    #[must_use]
    pub fn is_empty(self) -> bool {
        !self.clipboard && !self.primary
    }
}

/// An OSC 52 clipboard WRITE the child requested: set the named [`targets`](Self::targets) to
/// [`text`](Self::text) (already base64-decoded and UTF-8-validated by the parser, and clamped
/// to a byte cap — see [`crate::emulator`]). A clipboard CLEAR (`OSC 52` with no
/// data) arrives here as a write of the empty string. LATCHED like a [`Notification`] (last
/// wins) and paired with a monotonic sequence ([`VtPort::clipboard_write_seq`]) so a consumer
/// applies each write exactly once; it carries no cells, so it does not bump row damage.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct ClipboardWrite {
    /// The selections to set.
    pub targets: ClipboardTargets,
    /// The text to place on each target selection (may be empty — a clear).
    pub text: String,
}

/// An OSC 52 clipboard READ the child requested (`OSC 52 ; <sel> ; ?`): send it the current
/// contents of [`target`](Self::target). The terminal cannot answer from the emulator — the
/// clipboard is the display client's, and the answer is written back to the pane's PTY as an
/// `OSC 52 ; <sel> ; <base64> ST` reply (see [`crate::emulator::osc52_reply`]). LATCHED and
/// paired with a monotonic sequence ([`VtPort::clipboard_query_seq`]) so a consumer answers
/// each query once. A query naming both selections reduces to the clipboard (a reply carries
/// one selection); a query for none sprag models reduces to the clipboard too.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct ClipboardQuery {
    /// The selection whose contents the child asked for.
    pub target: ClipboardTarget,
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

/// The last shell command sliced from the OSC 133 [`PromptMark`]s — its line, its output, its
/// exit status, and whether it is still running. Returned by [`Screen::last_command`]. Serde-free:
/// the VT layer owns no wire shape; the host projects this to JSON for the `read_last_command` MCP
/// tool, exactly as it projects [`ShellState`] via [`ShellState::wire_str`].
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct LastCommand {
    /// The command line: the prompt row(s) from the [`Prompt`](PromptMark::Prompt) up to the
    /// [`Output`](PromptMark::Output) mark, INCLUDING the shell's prompt string (input-start `B` is
    /// not a row mark — a documented bound). Empty when integration began after the prompt (an
    /// `Output` with no preceding `Prompt`).
    pub command: String,
    /// The command's output: the rows from [`Output`](PromptMark::Output) to
    /// [`CommandEnd`](PromptMark::CommandEnd), or to the bottom of the pane while [`running`](Self::running).
    pub output: String,
    /// The reported exit status, or `None` for a bare `OSC 133 ; D` (finished, unreported) or while
    /// still running.
    pub exit_status: Option<i32>,
    /// `true` when no [`CommandEnd`](PromptMark::CommandEnd) has arrived after the output start — the
    /// command is still executing and `output` is what it has printed so far.
    pub running: bool,
}

/// One contiguous OSC-8 hyperlink run on the visible grid: the displayed text and the link it
/// points at. Returned by [`Screen::hyperlink_runs`]. Serde-free like [`LastCommand`] — the host
/// projects it to JSON for the `read_pane_links` MCP tool. The tmux-superior surface: an agent
/// reads a link's DESTINATION as data (the URI, without OCR), which `capture-pane` cannot give
/// because tmux flattens OSC 8 to plain text.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct LinkRun {
    /// The displayed text the link covers (its cells' clusters, in reading order).
    pub text: String,
    /// The URI the link opens (`https://…`, `file://…`, `mailto:…`).
    pub uri: String,
    /// The OSC-8 `id=` grouping key, or `None` for an anonymous link. Two runs that share an id are
    /// one logical link (a link split across a wrap, or the same target repeated).
    pub id: Option<String>,
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

/// An inline raster image the terminal is displaying (Kitty graphics / Sixel) — its decoded
/// RGBA pixels plus where it sits on the grid. sprag-owned and library-agnostic (no termwiz
/// types), exactly as [`Cell`] / [`Hyperlink`] are, so the VT-library choice stays reversible.
///
/// `rgba` is `width * height * 4` bytes (8-bit R,G,B,A row-major). `anchor` is the top-left
/// grid CELL the image is placed at (Kitty places at the cursor); a consumer converts it to
/// pixels via the cell metric and composites the image over the text grid. `id` is the Kitty
/// image id (`i=`), so a re-transmit under the same id REPLACES the image (animation / update).
///
/// Stage-1 scope (pinion R1404): the image is a static placement cleared on screen-clear /
/// alt-screen only; scroll / erase-covered-cells / reflow eviction is a later stage
/// (documented bound), as are chunked transmit, delete, query-ack, and PNG.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Image {
    /// The Kitty image id (`i=`); a re-transmit under the same id replaces this image.
    pub id: u32,
    /// Pixel width of the RGBA raster.
    pub width: u32,
    /// Pixel height of the RGBA raster.
    pub height: u32,
    /// `width * height * 4` bytes: 8-bit R,G,B,A, row-major.
    pub rgba: Vec<u8>,
    /// The top-left grid cell `(col, row)` the image is anchored at (the cursor at transmit).
    pub anchor: (u16, u16),
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
    /// Inline raster images (Kitty graphics / Sixel) the child is displaying, in transmit
    /// order (pinion R1404). Keyed by [`Image::id`] on insert (a re-transmit replaces). Carries
    /// no cells, so it is NOT parallel to the rows like [`Self::marks`]; it is cleared wholesale
    /// on screen-clear / alt-screen (Stage-1 lifecycle — scroll / reflow eviction is later).
    images: Vec<Image>,
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
            images: Vec::new(),
        }
    }

    /// The inline images (Kitty graphics / Sixel) the child is displaying (pinion R1404), in
    /// transmit order. A consumer composites each over the text grid at its [`Image::anchor`]
    /// cell (× the cell metric). Empty until the child transmits one.
    #[must_use]
    pub fn images(&self) -> &[Image] {
        &self.images
    }

    /// Add (or REPLACE, by [`Image::id`]) an inline image — a Kitty re-transmit under the same
    /// id updates it in place. Bounded by [`IMAGE_CAP`] against a child that streams distinct
    /// ids without clearing; past the cap the oldest is dropped (FIFO).
    pub(crate) fn add_image(&mut self, image: Image) {
        if let Some(slot) = self.images.iter_mut().find(|i| i.id == image.id) {
            *slot = image;
            return;
        }
        if self.images.len() >= IMAGE_CAP {
            self.images.remove(0);
        }
        self.images.push(image);
    }

    /// Drop every inline image — the screen-clear / alt-screen lifecycle (Stage 1).
    pub(crate) fn clear_images(&mut self) {
        self.images.clear();
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

    /// The last shell command — its line, output, and exit status — sliced from the OSC 133
    /// [`PromptMark`]s across scrollback and the visible grid, or `None` when no command has run
    /// under shell integration (no [`Output`](PromptMark::Output) mark; the caller then falls back
    /// to [`Self::full_text`]).
    ///
    /// The anchor is the most recent `Output` mark (`C`) — every command that produced output has
    /// one. The command line is the rows from the [`Prompt`](PromptMark::Prompt) that introduced it
    /// up to `C`; the output is the rows from `C` to the [`CommandEnd`](PromptMark::CommandEnd) `D`,
    /// or to the bottom while [`running`](LastCommand::running). This is a capability tmux's
    /// `capture-pane` lacks — a blind line range cannot slice one command's output; the marks give
    /// semantic boundaries. Bounded: it scans the retained lines (scrollback cap + visible rows).
    #[must_use]
    pub fn last_command(&self) -> Option<LastCommand> {
        let sb_len = self.scrollback.len();
        let total = sb_len + self.rows as usize;
        let mark_at = |i: usize| {
            if i < sb_len {
                self.scrollback_mark(i)
            } else {
                self.mark((i - sb_len) as u16)
            }
        };
        // The last output start anchors the last command (running or finished).
        let c = (0..total)
            .rev()
            .find(|&i| mark_at(i) == Some(PromptMark::Output))?;
        // Its end: the first CommandEnd after C; None => the command is still running.
        let d = (c + 1..total).find(|&i| matches!(mark_at(i), Some(PromptMark::CommandEnd(_))));
        // The prompt that introduced it: the last Prompt at or before C.
        let a = (0..=c)
            .rev()
            .find(|&i| mark_at(i) == Some(PromptMark::Prompt));

        let text = |range: std::ops::Range<usize>| -> String {
            let mut lines: Vec<String> = range
                .map(|i| {
                    if i < sb_len {
                        cells_text(&self.scrollback[i].cells)
                    } else {
                        self.row_text((i - sb_len) as u16)
                    }
                })
                .collect();
            while lines.last().is_some_and(String::is_empty) {
                lines.pop();
            }
            lines.join("\n")
        };

        let exit_status = match d {
            Some(i) => match mark_at(i) {
                Some(PromptMark::CommandEnd(status)) => status,
                _ => None,
            },
            None => None,
        };
        Some(LastCommand {
            command: a.map(|a| text(a..c)).unwrap_or_default(),
            output: text(c..d.unwrap_or(total)),
            exit_status,
            running: d.is_none(),
        })
    }

    /// The logical line indices (from the OLDEST retained line, `0`) of every OSC 133
    /// prompt-start ([`PromptMark::Prompt`]) mark — oldest first, across scrollback then the
    /// visible grid. These are the jump-to-prompt targets: a display client's scroll `offset_y`
    /// IS the view's top logical line (rows from the oldest), so jumping the view to prompt `L`
    /// is `scroll_to(L)`. Empty without shell integration. Bounded (scrollback cap + rows).
    #[must_use]
    pub fn prompt_positions(&self) -> Vec<usize> {
        let sb_len = self.scrollback.len();
        let mut out = Vec::new();
        for (i, line) in self.scrollback.iter().enumerate() {
            if line.mark == Some(PromptMark::Prompt) {
                out.push(i);
            }
        }
        for r in 0..self.rows {
            if self.mark(r) == Some(PromptMark::Prompt) {
                out.push(sb_len + r as usize);
            }
        }
        out
    }

    /// The OSC-8 hyperlink runs on the VISIBLE grid, in reading order — each a contiguous span of
    /// cells sharing one link ([`LinkRun`]). Adjacent cells with the same link handle (a link and
    /// its wrap continuations) form one run; a run ends where the link changes or stops. An agent
    /// reads this to learn a pane's clickable targets as DATA — the URIs without OCR, which tmux's
    /// `capture-pane` cannot expose (it flattens OSC 8 to plain text). Scans the visible rows only
    /// (the on-screen links, bounded by the grid size); a `Some(id)` on two runs ties them into one
    /// logical link for the consumer. Grouping is by the link's `Arc` POINTER, so two anonymous
    /// links to the same URI stay distinct runs (as they render).
    #[must_use]
    pub fn hyperlink_runs(&self) -> Vec<LinkRun> {
        let mut runs: Vec<LinkRun> = Vec::new();
        // The link the current run belongs to, by `Arc` pointer identity; `None` between runs.
        let mut open: Option<*const Hyperlink> = None;
        for row in 0..self.rows {
            for col in 0..self.cols {
                let Some(cell) = self.cell(col, row) else {
                    break;
                };
                match &cell.hyperlink {
                    Some(link) => {
                        let ptr = Arc::as_ptr(link);
                        if open == Some(ptr) {
                            // Continuation of the current run (including across a wrap).
                            if let Some(run) = runs.last_mut() {
                                run.text.push_str(&cell.cluster);
                            }
                        } else {
                            open = Some(ptr);
                            runs.push(LinkRun {
                                text: cell.cluster.clone(),
                                uri: link.uri.clone(),
                                id: link.id.clone(),
                            });
                        }
                    }
                    None => open = None,
                }
            }
        }
        runs
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
            // An inline image anchored on this row goes with the cleared content (R1404 Stage 3):
            // erase-in-display (ED, which clear_row's the affected rows) drops it, no ghost left.
            // During a scroll this is a no-op — images shift before the vacated rows are cleared.
            self.images.retain(|img| img.anchor.1 != row);
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
        // Inline images (Kitty / Sixel) carry across a resize verbatim — a plain
        // resize must NOT drop them (position-eviction under scroll / reflow is a
        // later stage, but the image data survives a re-layout).
        next.images = self.images.clone();
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
        // Inline images survive the rewrap verbatim: the RGBA is preserved and the anchor cell is
        // carried unchanged. Scroll now repositions an image (Stage 3), but a REFLOW does NOT
        // re-map the anchor to its rewrapped cell — a width change can leave an image misplaced
        // until the app redraws. A documented bound: precise reflow-repositioning of an image
        // (mirroring how a mark re-attaches to a re-broken line) is deferred; verbatim-carry keeps
        // the image rather than dropping or misrendering it. Same verbatim carry as [`Self::resized`]
        // (a plain resize, no rewrap, keeps anchors correct).
        next.images = self.images.clone();
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
        // An image tracks the grid like a cell: its anchor row scrolls up with the region. Do this
        // FIRST, before the row-clear below blanks the vacated rows (post-shift no image sits there,
        // so `clear_row`'s own image-drop is a no-op here). See [`Self::shift_images_up`].
        self.shift_images_up(top, bottom, n);
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
        // An image's anchor scrolls down with the region (mirror of [`Self::scroll_region_up`]).
        self.shift_images_down(top, bottom, n);
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

    /// Shift every inline image whose anchor row is in the scrolled region `[top, bottom]` UP by
    /// `n` — the image tracks its text (a sixel scrolls with the output, R1404 Stage 3). An image
    /// anchored in the `n` rows leaving the top of the region (`[top, top+n)`) is EVICTED, exactly
    /// as those rows' cells leave; an image outside the region is untouched. Anchor-granular (an
    /// image straddling the region boundary tracks by its anchor cell — a documented bound).
    /// Scrollback-image retention (re-appearing when you scroll back up) is a deferred bound: a
    /// scrolled-off-the-top image is dropped, not kept.
    fn shift_images_up(&mut self, top: u16, bottom: u16, n: u16) {
        self.images.retain_mut(|img| {
            let r = img.anchor.1;
            if r < top || r > bottom {
                true // outside the scrolled region — unmoved
            } else if r < top + n {
                false // in the rows leaving the top — evicted
            } else {
                img.anchor.1 = r - n;
                true
            }
        });
    }

    /// Shift every inline image whose anchor row is in `[top, bottom]` DOWN by `n`, evicting one
    /// that leaves the bottom — the mirror of [`Self::shift_images_up`] (RI / SD / IL).
    fn shift_images_down(&mut self, top: u16, bottom: u16, n: u16) {
        self.images.retain_mut(|img| {
            let r = img.anchor.1;
            if r < top || r > bottom {
                true
            } else if r + n > bottom {
                false // leaving the bottom of the region — evicted
            } else {
                img.anchor.1 = r + n;
                true
            }
        });
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

    /// The MOST RECENT OSC 52 clipboard WRITE the child requested ([`ClipboardWrite`]), or
    /// `None` if it never wrote one. LATCHED like [`notification`](Self::notification) and paired
    /// with [`clipboard_write_seq`](Self::clipboard_write_seq): a display client applies a write
    /// to its own system clipboard when the seq grows past the last it applied, so a late attach
    /// never re-clobbers the clipboard with a stale copy. Potentially large (a whole paste), so a
    /// consumer fetches it on demand off the seq rather than shipping it every poll.
    fn clipboard_write(&self) -> Option<&ClipboardWrite>;

    /// A monotonic counter bumped once per OSC 52 clipboard write the child requests — `0` before
    /// the first. A consumer that remembers the last value it applied learns a NEW write arrived
    /// when this grows (the latched payload alone cannot distinguish a re-write of the same text).
    fn clipboard_write_seq(&self) -> u64;

    /// The MOST RECENT OSC 52 clipboard READ the child requested ([`ClipboardQuery`]), or `None`
    /// if it never asked. LATCHED and paired with [`clipboard_query_seq`](Self::clipboard_query_seq):
    /// a display client answers a query — subject to its clipboard policy — when the seq grows,
    /// writing the reply back to the pane's PTY (see [`crate::emulator::osc52_reply`]). The answer
    /// is arbitrated to EXACTLY ONE reply across all attached clients (see the host).
    fn clipboard_query(&self) -> Option<ClipboardQuery>;

    /// A monotonic counter bumped once per OSC 52 clipboard read the child requests — `0` before
    /// the first. A consumer that remembers the last value it answered learns a NEW query arrived
    /// when this grows.
    fn clipboard_query_seq(&self) -> u64;

    /// Take (and clear) any bytes the terminal must write BACK to the PTY in reply to a query the
    /// child made — the device-response channel. Unlike a clipboard read (whose answer comes from
    /// the display client's clipboard), these are INTRINSIC responses the terminal answers itself:
    /// currently the Kitty keyboard `CSI ? u` flags query (`CSI ? flags u`). The layer driving
    /// [`advance`](Self::advance) drains this after each batch and writes the bytes to the child,
    /// which receives its reply as if the terminal typed it. Empty when the batch asked nothing.
    fn take_responses(&mut self) -> Vec<u8>;
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
