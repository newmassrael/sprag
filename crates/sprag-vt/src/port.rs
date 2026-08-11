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

use smol_str::SmolStr;
use unicode_width::UnicodeWidthChar;

use crate::history::{HistoryLimits, HistoryRow};

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
    // `UnicodeWidthChar::width` is exactly what `UnicodeWidthStr::width` sums per
    // char (control chars -> `None` -> 0), so this is behaviour-identical to the
    // old one-char-string form WITHOUT its per-call heap allocation — and the
    // print path calls this twice per printed char, so the alloc was a hot-path
    // throughput wall on bulk output.
    UnicodeWidthChar::width(ch).unwrap_or(0)
}

/// How many scrolled-off LOGICAL lines a [`Screen`] retains (FIFO) when nobody says otherwise —
/// the DEFAULT for [`Screen::history_limit`], not a ceiling over it.
///
/// A soft-wrapped line counts ONCE no matter how many physical rows it occupies, so history
/// retention is INDEPENDENT of the terminal width: narrowing (which multiplies physical rows) never
/// evicts history it would have kept at the wider size. This is tmux's `history-limit` model — tmux
/// counts logical lines because it never wraps history — and sprag matches it while ALSO reflowing,
/// which tmux does not.
///
/// The value each screen actually enforces is per-instance ([`Screen::new`]'s third argument),
/// because `history-limit` is a setting a user changes. This is what a pane born with nothing
/// configured gets, and `sprag-host`'s option table spells the same number as its own default — a
/// test there holds the two together, since nothing in the type system can.
pub const DEFAULT_SCROLLBACK_LINES: usize = 1000;

/// How many PHYSICAL rows a screen retaining `lines` logical lines may hold — a memory guard
/// orthogonal to the logical limit, derived from it rather than fixed.
///
/// A pathological single logical line (megabytes with no newline) is one logical line under the
/// logical limit yet many physical rows, so without this it could pin unbounded memory. Set
/// generously (a large multiple of the logical limit) so it only bites the runaway case; normal
/// content is bounded by the logical limit well below it. tmux has no such ceiling — this is where
/// sprag is stricter (tmux-superior on memory safety).
///
/// DERIVED per screen rather than a `const`, because a fixed ceiling computed from the DEFAULT
/// would silently cap a user who raised `history-limit`: at the old 8x1000, a pane configured for
/// 50,000 lines would stop at 8,000 physical rows and the setting would appear not to work. It
/// saturates rather than wrapping, so a limit near `usize::MAX` cannot fold back to a small ceiling.
pub(crate) const fn scrollback_physical_ceiling(lines: usize) -> usize {
    lines.saturating_mul(8)
}

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

/// Which default a [`Color::Default`] cell resolves to — foreground or
/// background have distinct defaults (OSC 10 vs OSC 11).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ColorTarget {
    Foreground,
    Background,
}

/// The terminal's live colour palette: the 256 indexed slots plus the three
/// dynamic colours (default foreground / background / cursor). This is the
/// SSOT the OSC colour commands mutate — `OSC 4` a palette index, `OSC 10 / 11
/// / 12` the dynamic colours, `OSC 104 / 110 / 111 / 112` the resets — and it
/// is what a colour QUERY (`OSC 4 ; i ; ?`, `OSC 10 ; ?`, …) reports back.
///
/// The palette lives on the [`Emulator`](crate::emulator::Emulator), NOT on a
/// [`Screen`]: it is a single terminal-wide state shared by the main and
/// alternate buffers (an alt-screen app inherits the terminal's palette, and an
/// `OSC 4` from either buffer is one global change), so it must survive the
/// whole-[`Screen`] swap the alt-screen transition performs.
///
/// Cells store their colour SYMBOLICALLY ([`Color::Indexed`] / [`Color::Default`]),
/// never a resolved RGB, so a palette change RE-COLOURS every existing cell that
/// uses it — the projection (sprag-grid's `project`, host-side) resolves each cell
/// against the live palette every frame. The seed values and resolution formulas
/// are the standard xterm palette, matching pinion's `Palette` byte-for-byte, so
/// an un-mutated palette projects identically to the pre-OSC-colour behaviour.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Palette {
    /// The 256 indexed slots (`0..=255`), each an explicit RGB so `OSC 4` can
    /// override ANY of them and `OSC 104` restores the xterm seed.
    colors: [Rgb; 256],
    default_fg: Rgb,
    default_bg: Rgb,
    /// The `OSC 12` cursor colour, or `None` while the child has set none.
    ///
    /// `Option`, unlike its `default_fg` / `default_bg` siblings, because the two facts
    /// "the child asked for this cursor colour" and "no cursor colour was asked for" render
    /// DIFFERENTLY and the renderer must be able to tell them apart: an unset cursor takes
    /// the colour of the cell it sits on (the reverse-video cursor xterm draws when its
    /// `cursorColor` resource is unset), while a set one is that absolute colour whatever it
    /// sits on. Collapsing the two — seeding a concrete RGB — would paint every cursor in the
    /// seed and lose the reverse-video default over coloured text. A default fg/bg has no such
    /// second reading: something must be drawn, so its seed IS the answer.
    ///
    /// One field carries both facts, so the value and its set-ness cannot drift apart.
    /// [`Self::cursor_color`] is the render read; [`Self::reported_cursor`] is the
    /// `OSC 12 ; ?` answer, which must name a colour even when unset.
    cursor: Option<Rgb>,
}

impl Palette {
    /// The standard xterm 16-colour ANSI base table (`0..=7` normal, `8..=15`
    /// bright) — the conventional default, identical to pinion's `XTERM_ANSI16`.
    const XTERM_ANSI16: [Rgb; 16] = [
        Rgb::new(0x00, 0x00, 0x00), // 0  black
        Rgb::new(0xcd, 0x00, 0x00), // 1  red
        Rgb::new(0x00, 0xcd, 0x00), // 2  green
        Rgb::new(0xcd, 0xcd, 0x00), // 3  yellow
        Rgb::new(0x00, 0x00, 0xee), // 4  blue
        Rgb::new(0xcd, 0x00, 0xcd), // 5  magenta
        Rgb::new(0x00, 0xcd, 0xcd), // 6  cyan
        Rgb::new(0xe5, 0xe5, 0xe5), // 7  white (light grey)
        Rgb::new(0x7f, 0x7f, 0x7f), // 8  bright black (grey)
        Rgb::new(0xff, 0x00, 0x00), // 9  bright red
        Rgb::new(0x00, 0xff, 0x00), // 10 bright green
        Rgb::new(0xff, 0xff, 0x00), // 11 bright yellow
        Rgb::new(0x5c, 0x5c, 0xff), // 12 bright blue
        Rgb::new(0xff, 0x00, 0xff), // 13 bright magenta
        Rgb::new(0x00, 0xff, 0xff), // 14 bright cyan
        Rgb::new(0xff, 0xff, 0xff), // 15 bright white
    ];

    /// The standard-xterm value for palette index `i`: the 16 ANSI base colours
    /// (`0..=15`), the 6x6x6 colour cube (`16..=231`), then the 24-step grayscale
    /// ramp (`232..=255`). The `OSC 104` reset restores an index to this.
    #[must_use]
    pub const fn xterm_indexed(i: u8) -> Rgb {
        match i {
            0..=15 => Self::XTERM_ANSI16[i as usize],
            16..=231 => {
                let n = i - 16; // 0..=215
                Rgb::new(
                    Self::cube_channel(n / 36),
                    Self::cube_channel((n / 6) % 6),
                    Self::cube_channel(n % 6),
                )
            }
            232..=255 => {
                // 24 grays: level = 8 + step*10  ->  8, 18, …, 238.
                let level = 8 + (i - 232) * 10;
                Rgb::new(level, level, level)
            }
        }
    }

    /// Map a cube axis step (`0..=5`) to its 8-bit channel value: `0` for step 0,
    /// then `55 + step*40` (→ `95, 135, 175, 215, 255`) — the xterm cube formula.
    const fn cube_channel(step: u8) -> u8 {
        if step == 0 { 0 } else { 55 + step * 40 }
    }

    /// The conventional xterm palette: the standard 256 indexed colours, with
    /// light-grey-on-black default foreground / background and a foreground-toned
    /// cursor — the seed a fresh terminal ships with.
    #[must_use]
    pub fn xterm_default() -> Self {
        let mut colors = [Rgb::new(0, 0, 0); 256];
        let mut i = 0usize;
        while i < 256 {
            colors[i] = Self::xterm_indexed(i as u8);
            i += 1;
        }
        Self {
            colors,
            default_fg: Self::XTERM_ANSI16[7],
            default_bg: Self::XTERM_ANSI16[0],
            cursor: None,
        }
    }

    /// Resolve a cell [`Color`] to the concrete RGB a painter draws — the ONLY
    /// place index / default resolution happens (the projection calls this per
    /// cell). `Default` consults the per-target dynamic colour, `Indexed` the
    /// palette slot, `Rgb` is used verbatim.
    #[must_use]
    pub fn resolve(&self, color: Color, target: ColorTarget) -> Rgb {
        match color {
            Color::Default => match target {
                ColorTarget::Foreground => self.default_fg,
                ColorTarget::Background => self.default_bg,
            },
            Color::Indexed(i) => self.colors[i as usize],
            Color::Rgb(rgb) => rgb,
        }
    }

    /// The current default foreground / background / cursor colour — the value an
    /// `OSC 10 / 11 / 12 ; ?` query reports.
    #[must_use]
    pub const fn default_fg(&self) -> Rgb {
        self.default_fg
    }
    #[must_use]
    pub const fn default_bg(&self) -> Rgb {
        self.default_bg
    }
    /// The explicit `OSC 12` cursor colour, or `None` while none is set — the RENDER read,
    /// projected as pinion's `GridCursor::cursor_color` (whose `None` means "take the cell's
    /// colour"). Distinct from [`Self::reported_cursor`], which must always name a colour.
    #[must_use]
    pub const fn cursor_color(&self) -> Option<Rgb> {
        self.cursor
    }

    /// The cursor colour an `OSC 12 ; ?` query reports: the explicit one when set, else the
    /// xterm seed. A query must answer with a colour — a terminal that has been asked nothing
    /// still draws a cursor — so this is the one place the unset case is given a value, and it
    /// is the value sprag has always reported.
    #[must_use]
    pub fn reported_cursor(&self) -> Rgb {
        self.cursor.unwrap_or(Self::XTERM_ANSI16[7])
    }
    /// The current colour of palette index `i` — the value an `OSC 4 ; i ; ?`
    /// query reports.
    #[must_use]
    pub const fn indexed(&self, i: u8) -> Rgb {
        self.colors[i as usize]
    }

    /// `OSC 10 / 11 / 12` set: replace the default foreground / background / cursor.
    pub const fn set_default_fg(&mut self, rgb: Rgb) {
        self.default_fg = rgb;
    }
    pub const fn set_default_bg(&mut self, rgb: Rgb) {
        self.default_bg = rgb;
    }
    pub const fn set_cursor(&mut self, rgb: Rgb) {
        self.cursor = Some(rgb);
    }
    /// `OSC 4 ; i ; spec` set: override palette index `i`.
    pub const fn set_indexed(&mut self, i: u8, rgb: Rgb) {
        self.colors[i as usize] = rgb;
    }

    /// `OSC 110 / 111 / 112` reset: restore the default foreground / background /
    /// cursor to the xterm seed.
    pub fn reset_default_fg(&mut self) {
        self.default_fg = Self::XTERM_ANSI16[7];
    }
    pub fn reset_default_bg(&mut self) {
        self.default_bg = Self::XTERM_ANSI16[0];
    }
    /// `OSC 112` returns the cursor to having NO explicit colour — back to the cell-derived
    /// render, not to the seed as a set value. That is what makes the reset undo an `OSC 12`
    /// rather than merely overwrite it with a colour that happens to look like the default.
    pub const fn reset_cursor(&mut self) {
        self.cursor = None;
    }
    /// `OSC 104 ; i` reset: restore palette index `i` to its xterm value.
    pub const fn reset_indexed(&mut self, i: u8) {
        self.colors[i as usize] = Self::xterm_indexed(i);
    }
    /// `OSC 104` (no params) reset: restore the entire indexed palette.
    pub fn reset_all_indexed(&mut self) {
        let mut i = 0usize;
        while i < 256 {
            self.colors[i] = Self::xterm_indexed(i as u8);
            i += 1;
        }
    }
}

impl Default for Palette {
    fn default() -> Self {
        Self::xterm_default()
    }
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
    ///
    /// A [`SmolStr`] rather than a `String`: a cluster is almost always a
    /// single char (<= 4 UTF-8 bytes), which stays inline in the cell with no
    /// heap allocation (the print path builds one per printed char — the
    /// bulk-output hot path). Clone is O(1) — an inline copy, or an `Arc` bump
    /// for the rare cluster past the inline cap — which the clone-heavy
    /// scrollback / reflow / selection paths get for free. It is effectively
    /// immutable; the one growth site (combining-mark merge) rebuilds it.
    pub cluster: SmolStr,
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
    /// The DECSCA (Select Character Protection Attribute) protection bit: `true`
    /// marks this cell as protected against the *selective* erase family (DECSED
    /// `CSI ? Ps J`, DECSEL `CSI ? Ps K`, DECSERA `CSI … $ {`), which skip
    /// protected cells and clear only the rest — the mechanism a form / dialog
    /// uses to blank the input fields while keeping the labels. It has NO visual
    /// effect (protection is invisible), so it is a bare `Cell` field rather than
    /// an [`Attrs`] flag: the projection maps only the rendered attributes, and a
    /// protected cell renders identically to an unprotected one. Emulator-internal
    /// — never serialized to the wire or read by the client. The *non*-selective
    /// erases (ED / EL / ECH / DECERA) ignore it and clear everything.
    pub protected: bool,
}

impl Default for Cell {
    fn default() -> Self {
        Self {
            cluster: SmolStr::new_inline(" "),
            fg: Color::Default,
            bg: Color::Default,
            underline_color: None,
            attrs: Attrs::default(),
            hyperlink: None,
            width: Width::Narrow,
            protected: false,
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
            cluster: SmolStr::new_inline(""),
            fg: head.fg,
            bg: head.bg,
            underline_color: head.underline_color,
            attrs: head.attrs,
            // The continuation column of a wide link glyph belongs to the same
            // OSC-8 link, so the whole glyph is one hover / activation target.
            hyperlink: head.hyperlink.clone(),
            width: Width::Trailer,
            // The trailer shares the head's DECSCA protection, so a selective
            // erase treats a wide glyph as one unit (both cells kept or both
            // cleared) rather than splitting it.
            protected: head.protected,
        }
    }
}

/// A cell row's text: clusters concatenated, trailing blanks trimmed. The ONE
/// row-to-text mapping shared by [`Screen::row_text`] (visible rows) and
/// [`Screen::scrollback_rows`] (scrolled-off rows), so the capture path and the
/// scrollback never drift. Wide trailers contribute `""`, blank cells `" "`.
#[must_use]
/// Collect `needle`'s matches in ONE line's `cells` into `out` — the per-line half of
/// [`Screen::find`]. `needle` must already be ASCII-lowercased; `text` / `starts` are the caller's
/// scratch buffers (cleared here, reused across lines). Returns `false` when [`FIND_MATCH_CAP`] was
/// reached, which is the caller's signal to stop scanning.
///
/// `starts` is the byte offset each CELL's cluster begins at, plus a one-past-the-end sentinel —
/// the map that turns a byte match back into COLUMNS. It has to exist because the two are not the
/// same axis: a wide cluster contributes its bytes to one cell and occupies two columns, and a
/// trailer contributes no bytes at all.
fn find_in_line(
    cells: LogicalLine<'_>,
    needle: &str,
    line: usize,
    text: &mut String,
    starts: &mut Vec<usize>,
    out: &mut Vec<FindMatch>,
) -> bool {
    let searchable = line_text(cells, text, starts);
    // Byte-length-preserving by construction (ASCII only), so every offset below stays valid — and
    // it cannot change which trailing bytes are whitespace, so `searchable` holds across it.
    text.make_ascii_lowercase();
    let mut from = 0;
    while from < searchable {
        let Some(offset) = text[from..searchable].find(needle) else {
            return true;
        };
        let start = from + offset;
        let end = start + needle.len();
        out.push(match_span(cells, starts, line, start, end));
        if out.len() >= FIND_MATCH_CAP {
            return false;
        }
        from = end; // non-overlapping: the next scan starts past this match
    }
    true
}

/// Append every non-overlapping match of `regex` in one line's cells to `out`, returning whether
/// the scan stayed within [`FIND_MATCH_CAP`]. The regex peer of [`find_in_line`], sharing its
/// byte-offset→column mapping so the two searches cannot disagree about where a match sits.
///
/// The line text is NOT case-folded: the pattern language owns that decision through `(?i)`, and
/// folding underneath it would overrule what the caller wrote. Zero-width matches are skipped —
/// they cover no cells, so a coordinate could not point at anything; `find_iter` still advances
/// past them, so a pattern that can match empty terminates.
fn regex_in_line(
    cells: LogicalLine<'_>,
    regex: &regex::Regex,
    line: usize,
    text: &mut String,
    starts: &mut Vec<usize>,
    out: &mut Vec<FindMatch>,
) -> bool {
    let searchable = line_text(cells, text, starts);
    for found in regex.find_iter(&text[..searchable]) {
        if found.start() == found.end() {
            continue;
        }
        out.push(match_span(cells, starts, line, found.start(), found.end()));
        if out.len() >= FIND_MATCH_CAP {
            return false;
        }
    }
    true
}

/// Fill the reused scratch buffers with one line's text and its cell→byte-offset map (plus a
/// sentinel past the last cell), returning the SEARCHABLE byte length.
///
/// The grid pads every row out to `cols` with blanks; searching that filler would let a space
/// needle match every row and let `$` anchor past the content, so the padding is excluded.
fn line_text(line: LogicalLine<'_>, text: &mut String, starts: &mut Vec<usize>) -> usize {
    text.clear();
    starts.clear();
    for share in line.0 {
        for cell in share.cells {
            starts.push(text.len());
            text.push_str(&cell.cluster);
        }
    }
    starts.push(text.len());
    text.trim_end().len()
}

/// The CELL span a matched byte range `start..end` covers — the conversion that makes a search
/// answer in columns rather than in byte offsets, which is the whole reason the search lives beside
/// the cells: a byte offset is not a column (a wide cluster is one cluster and two columns, and its
/// trailer contributes no bytes at all).
///
/// `cells` is the LOGICAL line, so the span this computes is in line coordinates; `shares` puts it
/// back on the grid ([`grid_span`]).
fn match_span(
    cells: LogicalLine<'_>,
    starts: &[usize],
    line: usize,
    start: usize,
    end: usize,
) -> FindMatch {
    // The cell holding the first matched byte: the LAST cell whose cluster starts at or before it
    // (a wide cluster's trailer shares its successor's offset, so the later entry wins and a match
    // is never attributed to a trailer).
    let col = starts.partition_point(|&begin| begin <= start).max(1) - 1;
    // Walk forward while the match's bytes run past the current cell's end...
    let mut cell = col;
    while cell < cells.len() && starts[cell + 1] < end {
        cell += 1;
    }
    // ...then absorb the trailer columns of a wide cluster the match ends on.
    let mut end_cell = cell + 1;
    while cells
        .cell(end_cell)
        .is_some_and(|c| c.width == Width::Trailer)
    {
        end_cell += 1;
    }
    grid_span(cells.0, line, col, end_cell)
}

/// One retained row's share of a logical line: which row it is, that row's cells, and where they
/// begin in the line. Built by [`Screen::scan_logical`] as it walks a line; read by [`line_text`]
/// to spell the line and by [`grid_span`] to put a match back onto the grid.
///
/// ⚠ **A BORROWED SLICE, NOT A COPY.** The line these describe is never materialised: a program
/// that prints a megabyte with no newline makes ONE logical line out of the whole scrollback, and
/// concatenating it would memcpy every [`Cell`] in the pane's history on every keystroke a find bar
/// types. Measured at 200x5000: **31.8 ms joined, 13.0 ms borrowed**. The rows are already
/// contiguous runs in the screen and the deque, so a slice each is all a reader needs.
///
/// `start` is the row's first cell counted along the LINE. No `end` and no column: a row's share
/// always begins at its column 0 — that is what a soft wrap is — and its length is the slice's.
#[derive(Clone, Copy, Debug)]
struct RowShare<'a> {
    row: usize,
    start: usize,
    cells: &'a [Cell],
}

/// The retained rows a logical line occupies, in order — the line as the search reads it, without
/// ever building it.
///
/// Indexing walks the shares rather than a flat buffer, which is why `start` is stored: the lookup
/// is a binary search over ROWS (a handful, or thousands for a pathological line) instead of a
/// scan. Only the match boundaries need it; the text pass is sequential.
#[derive(Clone, Copy, Debug)]
struct LogicalLine<'a>(&'a [RowShare<'a>]);

impl<'a> LogicalLine<'a> {
    /// The line's total width in cells.
    fn len(self) -> usize {
        self.0
            .last()
            .map_or(0, |last| last.start + last.cells.len())
    }

    /// The `index`-th cell of the line, or `None` past its end.
    fn cell(self, index: usize) -> Option<&'a Cell> {
        let share = self
            .0
            .get(self.0.partition_point(|s| s.start <= index).max(1) - 1)?;
        share.cells.get(index - share.start)
    }
}

/// Put the joined-line cell range `first..last` back onto the grid rows it covers, as the
/// [`FindMatch`] a client can highlight.
///
/// A match is contiguous in the LINE and therefore contiguous on the grid: it fills the rest of
/// its first row, every row between, and a prefix of its last. So the first row's `(col, cols)`
/// plus a width per following row describes it exactly, and a consumer that reads only the first
/// three fields highlights the visible head of the match instead of nothing.
///
/// The widths are carried rather than derived from the pane's width because they are not always
/// the pane's width: a wide cluster that will not fit at the margin wraps a column early, and a
/// DECLRMM region ends before the screen does. A consumer that divided by `cols` would paint one
/// cell too far on exactly the lines this round exists for.
///
/// ⚠ **THE HONEST COMPARISON FIRST: BOTH RIVALS SEARCHED ACROSS A SOFT WRAP BEFORE SPRAG DID.**
/// ghostty joins wrapped rows into its search window (`.unwrap = true`,
/// `src/terminal/search/sliding_window.zig:613` at `260288614`) and models the early-wrap pad as a
/// distinct cell (`spacer_head`); herdr keeps its line open across `row.soft_wrapped` and skips
/// `CellWide::SpacerHead` (`src/pane/terminal.rs:617` at `9a4ce5e1`). R344 catches up; it does not
/// pull ahead on the search.
///
/// What this shape does buy is that the answer is complete WITHOUT THE GRID. Both rivals resolve a
/// match's middle rows against the pane's width at paint time — ghostty's `Selection.contains` is
/// "if between the top/bottom, always good" for any column (`src/terminal/Selection.zig:284`) and
/// herdr's highlighter takes `end_col = inner_rect.width - 1` on every row but the last
/// (`src/ui/panes.rs:790`) — which is sound only while a wrapped row is full. sprag's client is in
/// ANOTHER PROCESS and never sees the cells, so deriving was never available to it; carrying the
/// emulator's own measurement is what makes the answer self-describing. (Read from their sources at
/// those pins, not run: what a one-cell overpaint LOOKS like in either renderer is not measured.)
fn grid_span(shares: &[RowShare<'_>], line: usize, first: usize, last: usize) -> FindMatch {
    let mut found: Option<FindMatch> = None;
    for share in shares {
        let offset = share.start;
        let end = offset + share.cells.len();
        // The rows this match touches: those whose share overlaps `first..last`.
        if first < end && last > offset {
            let from = first.max(offset);
            let to = last.min(end);
            let width = u16::try_from(to - from).unwrap_or(u16::MAX);
            match &mut found {
                None => {
                    found = Some(FindMatch {
                        line,
                        row: share.row,
                        col: u16::try_from(from - offset).unwrap_or(u16::MAX),
                        cols: width,
                        wrapped: Vec::new(),
                    });
                }
                Some(hit) => hit.wrapped.push(width),
            }
        }
    }
    // Unreachable by construction: a match covers at least one cell (an empty needle answers
    // early, and a zero-width regex match is skipped), so the walk always finds its first row.
    // The fallback highlights NOTHING at the line's head rather than mis-highlighting something.
    found.unwrap_or(FindMatch {
        line,
        row: line,
        col: 0,
        cols: 0,
        wrapped: Vec::new(),
    })
}

fn cells_text(cells: &[Cell]) -> String {
    let mut line = String::new();
    for cell in cells {
        line.push_str(&cell.cluster);
    }
    line.trim_end().to_string()
}

/// A logical line's text: every row's share of it, concatenated, with the blanks at its end
/// trimmed — the display half of a search answer ([`FindLine::text`]).
///
/// Built from the borrowed shares rather than from a joined buffer, for [`RowShare`]'s reason: only
/// a line that MATCHED is ever spelled, so a pathological line costs a string once instead of a
/// cell copy on every keystroke.
fn shares_text(shares: &[RowShare<'_>]) -> String {
    let mut line = String::new();
    for share in shares {
        for cell in share.cells {
            line.push_str(&cell.cluster);
        }
    }
    line.truncate(line.trim_end().len());
    line
}

/// The cells of ONE retained row that belong to its logical line — the single definition of "this
/// row's share of the line", used by all three readers of a logical line: the reflow
/// ([`Screen::reflowed`]), the history encoder ([`crate::history::encode`]) and the search
/// ([`Screen::scan_logical`]).
///
/// `continues` is the row's soft-wrap continuation ([`Screen::continues`]):
///
/// * `Some(n)` — the line runs on, and put `n` cells here. The columns from `n` are LAYOUT: the pad
///   a wide cluster leaves when it will not fit at the margin, or a column outside a DECLRMM
///   region. They are dropped, and the blanks BEFORE `n` are kept — a wrapped row's trailing space
///   is a space the child printed, and the next row's text follows it directly.
/// * `None` — the line ends on this row, so its run out to the margin is the grid's padding and is
///   trimmed. "Blank" is full [`Cell`] equality, not a space cluster: a trailing run carrying a
///   BACKGROUND colour is a coloured bar the user can see, so it is content and stays.
///
/// Both halves have cost a defect. Trimming a CONTINUING row deletes printed spaces (R343's
/// drop-paste fold); keeping a continuing row's whole width injects the wide-cluster pad into the
/// user's text (`reflow_drops_the_pad_a_wide_cluster_left_at_the_margin`). One reader, so a fix to
/// either reaches the reflow, the durable history and the search together.
pub(crate) fn line_cells(cells: &[Cell], continues: Option<u16>) -> &[Cell] {
    match continues {
        Some(upto) => &cells[..(upto as usize).min(cells.len())],
        None => {
            let blank = Cell::blank();
            match cells.iter().rposition(|cell| *cell != blank) {
                Some(last) => &cells[..=last],
                None => &[],
            }
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
    /// Whether the cursor is a BLINKING variant — the DECSCUSR blink axis (`1`/`3`/`5` blinking,
    /// `2`/`4`/`6` steady) and the legacy mode-12 toggle, which write this one state.
    ///
    /// The MODE, never the render-time on/off phase: the phase belongs to the last renderer in
    /// the chain (pinion's own per-window blink clock), and folding it in here would make
    /// [`visible`](Self::visible) — pure DECTCEM, "does the app want a cursor at all" —
    /// unreadable, since a consumer could no longer tell an app-hidden cursor from a blink's
    /// off-half. Two facts, kept apart.
    pub blink: bool,
}

impl Default for Cursor {
    fn default() -> Self {
        Self {
            col: 0,
            row: 0,
            shape: CursorShape::Block,
            visible: true,
            blink: false,
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
    /// Bracketed paste (DEC private mode 2004). When set, the child has asked to receive PASTED
    /// text wrapped in `ESC [ 200 ~` … `ESC [ 201 ~` so it can tell a paste from typed keystrokes
    /// (shells / editors enable it so a multi-line paste does not auto-execute line by line). The
    /// paste seam at the PTY boundary consults this flag to decide whether to bracket; typed and
    /// IME-committed text is never bracketed (it is not a paste). Off by default (raw paste).
    pub bracketed_paste: bool,
    /// Which pointer events the child has asked the terminal to REPORT (the DECSET mouse-tracking
    /// modes 1000 / 1002 / 1003). While active the terminal stops handling the mouse itself
    /// (selection, wheel-scroll) and instead forwards reports to the child. The sprag-owned mouse
    /// encoder gates each event against this; the display client reads it to decide whether to
    /// capture the pointer. Off by default ([`MouseProtocol::None`] — the terminal owns the mouse).
    pub mouse_protocol: MouseProtocol,
    /// How a mouse report serializes on the wire (DECSET 1006). Independent of
    /// [`mouse_protocol`](Self::mouse_protocol): a child sets a tracking mode AND, optionally, an
    /// encoding. Defaults to the legacy [`MouseEncoding::X10`]; `ESC [ ? 1006 h` selects the modern
    /// [`MouseEncoding::Sgr`] form (unbounded coordinates, a distinct release edge).
    pub mouse_encoding: MouseEncoding,
    /// Focus reporting (DEC private mode 1004). When set, the child has asked the terminal to send
    /// `ESC [ I` when the terminal (here: the pane) GAINS focus and `ESC [ O` when it LOSES focus,
    /// so an app (vim checking for external file changes, a TUI dimming when inactive) can react.
    /// The display client emits the edge on a pane focus change; the encode at the PTY boundary
    /// consults this flag. Off by default (no focus reports).
    pub focus_tracking: bool,
    /// LNM line-feed / new-line mode (ANSI mode 20). When set, a received LF / VT / FF also returns
    /// the cursor to column 0 (a CR+LF, applied by the emulator's control handler), AND the Return
    /// key transmits CR+LF instead of a bare CR (applied by the key encoder). Off by default (a bare
    /// LF moves straight down, Return sends CR) — the normal Unix behaviour. The one flag both halves
    /// read, so the display translation and the key encoding can never disagree.
    pub newline_mode: bool,
}

/// Which pointer events a child has asked the terminal to report, selected by the DECSET
/// mouse-tracking modes. The variants are ordered by how much they report: each reports a superset
/// of the events below it, so a single field captures the effective reporting level (a child sets
/// exactly one tracking mode in practice; the pathological "several set at once, reset the highest"
/// nuance of xterm's independent mode bits is a documented bound — see the emulator's `mode`).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum MouseProtocol {
    /// No reporting — the terminal owns the mouse (text selection, wheel scrolls the scrollback).
    #[default]
    None,
    /// DECSET 1000 (X11 mouse tracking): button PRESS and RELEASE only — no motion, no drag.
    Click,
    /// DECSET 1002 (button-event tracking): press/release + DRAG (motion while a button is held).
    ButtonEvent,
    /// DECSET 1003 (any-event tracking): press/release + ALL motion (whether or not a button is held).
    AnyEvent,
}

impl MouseProtocol {
    /// Whether any reporting is active (the terminal should forward pointer events to the child
    /// rather than handle them itself). `false` only for [`MouseProtocol::None`].
    #[must_use]
    pub fn is_active(self) -> bool {
        !matches!(self, Self::None)
    }

    /// Whether this level reports pointer MOTION with no button held (only [`MouseProtocol::AnyEvent`]).
    #[must_use]
    pub fn reports_motion(self) -> bool {
        matches!(self, Self::AnyEvent)
    }

    /// Whether this level reports DRAG — motion while a button is held ([`MouseProtocol::ButtonEvent`]
    /// and [`MouseProtocol::AnyEvent`]).
    #[must_use]
    pub fn reports_drag(self) -> bool {
        matches!(self, Self::ButtonEvent | Self::AnyEvent)
    }

    /// The wire / display token for an ACTIVE tracking level (`"click"` / `"button"` / `"any"`), or
    /// `None` for [`None`](MouseProtocol::None). The single source of the wire vocabulary — a
    /// serializer omits the key when this is `None`, so a pane not tracking the mouse keeps the
    /// pre-mouse wire shape (additive), and a display client reads it to decide whether to capture
    /// the pointer (and, from the level, whether to forward drag / motion).
    #[must_use]
    pub fn wire_str(self) -> Option<&'static str> {
        match self {
            Self::None => None,
            Self::Click => Some("click"),
            Self::ButtonEvent => Some("button"),
            Self::AnyEvent => Some("any"),
        }
    }

    /// The inverse of [`wire_str`](Self::wire_str) — the SSOT for reading a wire `mouse` token back
    /// into a level. A missing key (`None`) or an unknown token is [`MouseProtocol::None`] (the
    /// pane is not tracking / an older daemon), so a display client parses the level a producer
    /// serialized without duplicating the vocabulary.
    #[must_use]
    pub fn from_wire_str(token: Option<&str>) -> Self {
        match token {
            Some("click") => Self::Click,
            Some("button") => Self::ButtonEvent,
            Some("any") => Self::AnyEvent,
            _ => Self::None,
        }
    }
}

/// How a mouse report is serialized to the child. The legacy [`MouseEncoding::X10`] form packs the
/// button and 1-based coordinates into three `32 + value` bytes after `ESC [ M` (so a coordinate
/// past column/row 223 cannot be represented — it is clamped); the modern [`MouseEncoding::Sgr`]
/// form (DECSET 1006) writes decimal parameters `ESC [ < b ; col ; row` with the final byte `M` for
/// a press/motion and `m` for a release, so coordinates are unbounded and the released button is
/// preserved.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum MouseEncoding {
    /// The legacy `ESC [ M` + three `32 + value` bytes form (the default before DECSET 1006).
    #[default]
    X10,
    /// The DECSET 1006 `ESC [ < b ; col ; row M|m` form.
    Sgr,
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
    /// How much the CHILD said this matters — [`Urgency::Normal`] when it could not say.
    pub urgency: Urgency,
}

crate::closed_set! {
    /// How much a child says its own [`Notification`] matters — the `u=` key of kitty's
    /// `OSC 99` desktop-notification protocol, in its own order.
    ///
    /// # Why the emulator models this at all
    ///
    /// Because it is the ONE thing in the whole notification path that the child can say and no
    /// other layer can guess. A multiplexer reading `build finished` off `OSC 9` has no way to know
    /// whether that sentence may scroll past unread; a child raising `u=2` has said it may not.
    /// Everything downstream — how long a surface holds the words, whether it waits for a person —
    /// is a projection of this fact, and a projection cannot recover information the capture threw
    /// away.
    ///
    /// # The default is NORMAL, and it is the protocol's, not a guess
    ///
    /// kitty specifies `u=1` (normal) for a notification that omits the key, so a chunk with no `u`
    /// has said *normal* rather than said nothing. The other two OSC forms are different:
    /// `OSC 9` and `OSC 777;notify` have no urgency in their grammar at all, so a child using them
    /// has made no claim, and this type's [`Default`] is what they get — which is the same value,
    /// arrived at for a different and stated reason.
    #[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Default)]
    pub enum Urgency {
        /// `u=0` — background information; miss it and nothing is lost.
        Low,
        /// `u=1`, and what a child that did not say gets.
        #[default]
        Normal,
        /// `u=2` — the child says a person is needed.
        Critical,
    }
}

impl Urgency {
    /// The urgency kitty's `u=<digit>` names, or [`None`] for a digit this protocol does not
    /// define.
    ///
    /// DERIVED by walking [`ALL`](Self::ALL) against [`digit`](Self::digit) rather than by a second
    /// `match`, so the two directions cannot come to disagree — [`crate::port::MouseEncoding`]'s
    /// discipline, and the reason the closed set is declared with the enum.
    #[must_use]
    pub fn parse(digit: &[u8]) -> Option<Self> {
        Self::ALL.into_iter().find(|kind| kind.digit() == digit)
    }

    /// The `u=` digit this urgency is spelled with, as the bytes an OSC carries.
    #[must_use]
    pub const fn digit(self) -> &'static [u8] {
        match self {
            Self::Low => b"0",
            Self::Normal => b"1",
            Self::Critical => b"2",
        }
    }

    /// This urgency's NAME, for the surfaces that spell the scale in words rather than in digits.
    ///
    /// The same three names kitty's own protocol documents its digits with, and the same three the
    /// freedesktop desktop-notification specification uses — which is why a windowed client can
    /// hand this straight to a notifier while a terminal client sends [`digit`](Self::digit). One
    /// scale, two renderings of it, and neither front inventing a third.
    #[must_use]
    pub const fn word(self) -> &'static str {
        match self {
            Self::Low => "low",
            Self::Normal => "normal",
            Self::Critical => "critical",
        }
    }
}

crate::closed_set! {
    /// Which system selection an OSC 52 clipboard operation addresses. A windowing system
    /// distinguishes two, and sprag models both: the CLIPBOARD (the explicit Ctrl-C / Ctrl-V
    /// buffer, OSC 52 `c`) and the PRIMARY selection (X11 select-to-copy / middle-click paste,
    /// OSC 52 `p`). The OSC 52 X cut buffers (`0`-`9`) have no windowing-system analog and are
    /// not modeled; the "configured selection" `s` and the empty-`Pc` default fold onto the
    /// clipboard (the common intent — see [`crate::emulator`]).
    ///
    /// A [`closed_set!`](crate::closed_set!) because the pane surface's `clipboard_answer` action
    /// takes this as its `sel` argument and now PUBLISHES the two words
    /// ([`WIRE_WORDS`](Self::WIRE_WORDS)) — which the host used to match as bare literals, so a
    /// client had to know them out of band.
    #[derive(Clone, Copy, PartialEq, Eq, Debug)]
    pub enum ClipboardTarget {
        /// The system clipboard (OSC 52 `c`).
        Clipboard,
        /// The PRIMARY selection (OSC 52 `p`).
        Primary,
    }
}

impl ClipboardTarget {
    /// The OSC 52 selection character as a WORD — `"c"` / `"p"` — the spelling a `clipboard_answer`
    /// action's `sel` carries, and the one definition of both.
    ///
    /// A `&'static str` rather than a `char` because a published vocabulary is an array of words,
    /// and [`osc_char`](Self::osc_char) is derived from this rather than matching a second time.
    #[must_use]
    pub const fn wire_str(self) -> &'static str {
        match self {
            Self::Clipboard => "c",
            Self::Primary => "p",
        }
    }

    /// The selection a `sel` word names, or [`None`] for a word neither selection spells.
    ///
    /// ⚠ This is the WIRE argument's vocabulary, exactly two words. It is deliberately NOT the OSC 52
    /// `Pc` parse, which is richer: an `s` or an empty `Pc` from a CHILD folds onto the clipboard
    /// (see [`crate::emulator`]), because a terminal is lenient with what a program sends it. A
    /// client of sprag's own wire is not a program sprag has to tolerate, and the grammar it reads
    /// says two words.
    #[must_use]
    pub fn from_wire(word: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|it| it.wire_str() == word)
    }

    /// The OSC 52 selection character (`c` / `p`). A read reply echoes the requested selection
    /// so the asking app matches the response to its query.
    ///
    /// DERIVED from [`wire_str`](Self::wire_str) rather than matched again — one spelling, two
    /// shapes, so a selection cannot be one character on the reply and another on the wire.
    #[must_use]
    pub fn osc_char(self) -> char {
        char::from(self.wire_str().as_bytes()[0])
    }
}

crate::wire_words!(ClipboardTarget: wire_str);

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
    /// The command line: the prompt LINES from the [`Prompt`](PromptMark::Prompt) up to the
    /// [`Output`](PromptMark::Output) mark, INCLUDING the shell's prompt string (input-start `B` is
    /// not a row mark — a documented bound). Empty when integration began after the prompt (an
    /// `Output` with no preceding `Prompt`).
    pub command: String,
    /// The command's output: the LINES from [`Output`](PromptMark::Output) to
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

/// One literal match of a search needle in a pane's retained output. Returned (with its siblings)
/// by [`Screen::find`]. Serde-free like [`LastCommand`] — the host projects it to JSON.
///
/// ## A LINE and a ROW are different things, and a match knows both
///
/// A pane holds rows; a person reads lines. A line longer than the pane is wide occupies several
/// consecutive rows, so a match on it can START on one row and END on another — and until R344
/// this type could not say that, which is why the search did not look for such a match at all.
///
/// * [`line`](Self::line) names the LOGICAL line — by the retained row it begins on, which is the
///   axis [`Screen::prompt_positions`] reports and a display client's scroll `offset_y` speaks. It
///   is the join key to the [`FindLine`] carrying this line's text.
/// * [`row`](Self::row) is where the match itself starts, and [`col`](Self::col) /
///   [`cols`](Self::cols) are its CELL columns THERE — not byte or char offsets, so a highlight
///   lays straight onto the grid: a wide (double-width) cluster counts TWO columns, and a match
///   ending on one includes its trailer column.
/// * [`wrapped`](Self::wrapped) is the rest of the match, one width per row it runs on to. Empty
///   for the ordinary match that fits on its row.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct FindMatch {
    /// The LOGICAL line this match is in, named by the retained row it begins on (`0` = the oldest
    /// retained row; scrollback first, then the visible grid). The join key to [`FindLine::line`].
    pub line: usize,
    /// The retained row the match's FIRST cell sits on — [`line`](Self::line) plus however many
    /// wraps the match starts past, so equal to `line` unless it begins on a continuation row.
    /// This is the row a view scrolls to, and the row [`col`](Self::col) is a column of.
    pub row: usize,
    /// The starting CELL column within [`row`](Self::row).
    pub col: u16,
    /// The match's width in CELL columns ON [`row`](Self::row) (a wide cluster counts two) — the
    /// WHOLE match only when [`wrapped`](Self::wrapped) is empty.
    pub cols: u16,
    /// The match's width in cell columns on each row AFTER [`row`](Self::row), in order: a match
    /// that wraps covers rows `row + 1 ..= row + wrapped.len()`, each from column 0, because that
    /// is what a soft wrap is. Empty for a match that lies within one row.
    ///
    /// Carried rather than derived from the pane's width: a wide cluster that will not fit at the
    /// margin wraps a column EARLY, so a row of a wrapped line is not always full.
    pub wrapped: Vec<u16>,
}

/// One LOGICAL line that carries at least one match, with its text — the DISPLAY view of a search,
/// beside the coordinate view [`FindResult::matches`] gives.
///
/// Deduped: a line with three matches appears ONCE. That is the whole reason this is a second
/// collection rather than a `text` field on every [`FindMatch`] — a grep-like consumer prints one
/// line per matching LINE (ripgrep groups its submatches the same way), while a find bar navigates
/// matches one at a time and needs no text at all. Each consumer reads exactly one of the two.
///
/// The text is the whole logical line, so a line that wrapped over three rows is ONE entry reading
/// as the person reads it. That is what a `grep`-shaped consumer wants and what an agent quotes:
/// printing the rows separately would hand back a word broken in half, which is the same blindness
/// one layer up from the one the search itself had.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct FindLine {
    /// The logical line, named by the retained row it begins on — the join key back to
    /// [`FindMatch::line`].
    pub line: usize,
    /// The line's text: every row it occupies, joined, with the blanks at its end trimmed.
    pub text: String,
}

/// The answer to a [`Screen::find`]: the matches, oldest line first, and whether the search hit its
/// cap. Serde-free like [`LastCommand`]; the host projects it to JSON.
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct FindResult {
    /// Every match found, in reading order (oldest line first, then by column).
    pub matches: Vec<FindMatch>,
    /// Every line that carries a match, in the same order and each ONCE — the display view. Every
    /// `matches[i].line` appears here exactly once; the two are produced together, so a consumer can
    /// join on the line index without checking.
    pub lines: Vec<FindLine>,
    /// `true` when the search stopped at [`FIND_MATCH_CAP`] — the answer is complete only up to the
    /// last match reported, and lines after it were never scanned. Reported rather than silently
    /// implied: a capped answer that looked total would misdraw a match count.
    pub truncated: bool,
}

/// The most matches one [`Screen::find`] reports. A search is bounded because a one-character needle
/// over a full scrollback can match hundreds of thousands of times, and no consumer needs that: a
/// find bar navigates matches one at a time and a highlight only paints the visible ones. The cap is
/// far above any real navigation need, and [`FindResult::truncated`] says when it bit.
pub const FIND_MATCH_CAP: usize = 1000;

/// The compiled-program size a [`Screen::find_regex`] pattern may occupy before it is refused.
///
/// The engine's linear-time matching guarantee bounds how long a SEARCH can take, but not how long
/// COMPILING one can take: a deeply nested pattern with large bounded repetitions
/// (`(a{100}){100}{100}`) expands into an enormous program before it ever matches a byte. A search
/// is an interactive read served on the dispatch thread, so that bound is made explicit here rather
/// than inherited from the engine's much larger default. Generous beyond any hand-written pattern —
/// it bites the pathological case only, and reports it as a [`BadPattern`] rather than a stall.
pub const REGEX_SIZE_LIMIT: usize = 1 << 20;

/// A regular expression [`Screen::find_regex`] refused, carrying the engine's own explanation.
///
/// The message is the point: "unclosed group" and "regex exceeds size limit" tell a caller WHERE
/// their pattern went wrong, which a bare "no matches" or a `Null` could not. That is also why an
/// invalid pattern is not modelled as an absent answer on the wire — it is a well-formed address
/// whose value the engine rejected, and the caller needs to be told which.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct BadPattern(String);

impl BadPattern {
    /// The engine's explanation of why the pattern was refused.
    #[must_use]
    pub fn message(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for BadPattern {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for BadPattern {}

/// One scrolled-off line: its STYLED cells, the soft-wrap flag it carried, plus any
/// shell-integration [`PromptMark`] the row held. Bundling all three WITH the cells (rather
/// than parallel deques) makes them impossible to desync as lines are pushed, popped at the
/// retention limit, reflowed, or cloned on resize — the single-source-of-truth shape for
/// scrollback history.
#[derive(Clone, PartialEq, Eq, Debug)]
pub(crate) struct ScrollbackLine {
    pub(crate) cells: Vec<Cell>,
    /// The row's soft-wrap continuation ([`Screen::continues`]) when it scrolled off (or was
    /// rewrapped into) history: `Some(n)` means this history line's logical line CONTINUES onto
    /// the next and put `n` cells on this row. Preserved so [`Screen::reflowed`] can rebuild the
    /// scrolled-off logical lines and rewrap them to a new width — without it a resize could only
    /// carry scrollback verbatim (the width-stale-history bound this closes).
    pub(crate) continues: Option<u16>,
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
    /// `width * height * 4` bytes: 8-bit R,G,B,A, row-major. May be EMPTY in a wire SUMMARY —
    /// a display client carries only `{id,width,height,anchor,seq}` per poll and fetches the
    /// bytes ON DEMAND (R1404 Stage 5); [`Screen::images`] always carries them.
    pub rgba: Vec<u8>,
    /// The top-left grid cell `(col, row)` the image is anchored at (the cursor at transmit).
    pub anchor: (u16, u16),
    /// A monotonic CONTENT generation, assigned by `Screen::add_image` on every insert — a
    /// re-transmit that REPLACES the same [`id`](Self::id) gets a NEW `seq`. A display client keys
    /// its RGBA cache on `(id, seq)`, so it re-fetches the bytes exactly once per content change and
    /// reuses the cached decode otherwise (R1404 Stage 5 on-demand transport).
    pub seq: u64,
}

/// The scrolling region: the INCLUSIVE, 0-based RECTANGLE that vertical scrolls and the
/// line / character edits act inside.
///
/// The vertical extent is DECSTBM (`CSI Pt ; Pb r`); the horizontal extent is DECSLRM
/// (`CSI Pl ; Pr s`), settable only while DECLRMM — DEC private mode 69 — is on. They are ONE
/// value rather than two independent pairs because every scrolling primitive needs all four
/// bounds: a caller that passed the rows and forgot the columns would silently scroll the full
/// width, and that is the bug this type exists to make unrepresentable.
///
/// Defaults to the whole screen ([`Self::full`]) — what RIS, DECSTR, a resize, an alt-screen
/// transition and a DECLRMM reset all restore. A region is always non-empty and in-bounds:
/// `top <= bottom < rows` and `left <= right < cols`, which the setters enforce by REFUSING an
/// inverted or degenerate request rather than clamping it into a different rectangle.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ScrollRegion {
    pub(crate) top: u16,
    pub(crate) bottom: u16,
    pub(crate) left: u16,
    pub(crate) right: u16,
}

impl ScrollRegion {
    /// The whole screen — the power-on region, and what every reset restores. A zero-sized
    /// screen degenerates to `[0,0] x [0,0]`; the primitives all bail on `rows == 0 || cols == 0`
    /// before reading it, so the value is inert rather than a source of underflow.
    pub(crate) fn full(cols: u16, rows: u16) -> Self {
        Self {
            top: 0,
            bottom: rows.saturating_sub(1),
            left: 0,
            right: cols.saturating_sub(1),
        }
    }

    /// Whether the region spans every column of a `cols`-wide screen. This is the discriminator
    /// between the WHOLE-ROW fast paths (a slice rotation, allocation-free) and the BANDED ones,
    /// and it also gates the two behaviours that only make sense for a full-width scroll: history
    /// retention (a partial row leaving the top is not a scrollback line) and the soft-wrap /
    /// prompt-mark metadata moving with the rows (both are per-ROW facts, and a banded scroll
    /// moves only part of a row).
    pub(crate) fn full_width(&self, cols: u16) -> bool {
        self.left == 0 && self.right == cols.saturating_sub(1)
    }

    /// Whether `col` lies within the horizontal margins. A cursor OUTSIDE them makes the
    /// line-shaped edits (IL / DL / ICH / DCH) and the scroll-triggering index / reverse-index
    /// no-ops — the VT510 rule, so an app that parks the cursor outside its own margins cannot
    /// disturb the region.
    pub(crate) fn contains_col(&self, col: u16) -> bool {
        col >= self.left && col <= self.right
    }

    /// Whether `row` lies within the vertical margins.
    pub(crate) fn contains_row(&self, row: u16) -> bool {
        row >= self.top && row <= self.bottom
    }
}

/// A queryable terminal screen: a `cols x rows` grid of cells plus the
/// cursor, screen kind, and per-row damage generations.
///
/// This is the authoritative terminal state sprag owns (DESIGN.md §3:
/// the producer owns state; pinion is a projection). A VT backend fills
/// it; the projection reads it.
/// What a reader learned from [`Screen::lines_since`]: the lines, where to resume, and how many it
/// was too late for.
///
/// A type rather than a tuple because [`lost`](Self::lost) is the field a caller most wants to
/// forget and least can afford to — a silent gap in a relay is indistinguishable from a quiet
/// source, and this crate has paid for that confusion in other shapes already.
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct LinesSince {
    /// The complete logical lines after the cursor, oldest first, joined across the rows the
    /// terminal wrapped them onto.
    pub lines: Vec<String>,
    /// The cursor to pass next time — the address just past the last line yielded.
    pub next: u64,
    /// How many complete lines were shed and evicted before this reader asked for them. `0` in the
    /// ordinary case; non-zero means the source outran the retained history.
    pub lost: u64,
    /// The line the pane is STILL WRITING — everything after the last complete one, empty when
    /// there is nothing in progress.
    ///
    /// # ⚠⚠ Not part of [`lines`](Self::lines), and a consumer must earn the right to use it
    ///
    /// This is half a sentence. The child has not said it is finished, so acting on it means acting
    /// on something that may be about to change — and it is deliberately NOT counted by
    /// [`next`](Self::next), so a reader that ignores it loses nothing and a reader that takes it
    /// will be handed it again, whole, once the child ends the line.
    ///
    /// **The one case where it IS final is when the child has EXITED**: an unfinished line at EOF
    /// is unfinished forever, and a consumer that dropped it would silently lose the last thing the
    /// program said — which for a one-shot tool is usually its entire ANSWER, since a reply need
    /// not end in a newline. That is the only reading this crate sanctions, and the caller must
    /// establish the EOF itself; nothing here can.
    ///
    /// ⚠ A prompt (`> `) is the ordinary NON-terminal case: it sits here forever and relaying it
    /// would be relaying furniture. Waiting is the correct answer, and it costs nothing.
    pub partial: String,
}

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
    /// Rows scrolled off the top of the MAIN screen, oldest first, bounded by this screen's
    /// [`history_limit`](Self::history_limit) (FIFO). Each is the row's STYLED cells (fg/bg/attrs/
    /// width preserved), trailing blanks trimmed — so scrolled-back history paints
    /// with its original colors, not flattened to plain text. The text capture
    /// path derives strings from these cells ([`Screen::scrollback_rows`] /
    /// [`Screen::full_text`]), so there is one source; the grid projection reads
    /// the cells ([`Screen::scrollback_cells`]). Lines ARE reflowed on resize: [`Self::reflowed`]
    /// rebuilds the scrolled-off logical lines (via each line's bundled soft-wrap flag) and
    /// rewraps them to the new width, so history is not frozen at its old margin. Each line also
    /// carries any shell-integration [`PromptMark`] its row held (all bundled in [`ScrollbackLine`]
    /// so none can desync from the cells), so a prompt that scrolls into history stays a jump target.
    scrollback: VecDeque<ScrollbackLine>,
    /// Per-row soft-wrap continuation (the DEC `LINE_WRAPPED` attribute, plus WHERE it wrapped):
    /// `continues[r] == Some(n)` means row `r`'s logical line CONTINUES onto row `r + 1` and put
    /// `n` cells on this row, so a reflow ([`Self::reflowed`]) joins `cells[..n]` to the next row
    /// before re-breaking to a new width. Without it a resize cannot tell a soft wrap from a hard
    /// newline, so it cannot rewrap (the verbatim [`Self::resized`] fallback leaves a live shell's
    /// per-width prompt redraws stacked up).
    ///
    /// ⚠ **THE COLUMN IS NOT DECORATION ON THE FLAG — IT IS THE HALF A `bool` LOST.** A wrapped
    /// row is USUALLY full, but not always: a wide cluster that will not fit at the margin wraps
    /// EARLY, leaving a column the emulator never wrote, and under DECLRMM the line ends at the
    /// region's right margin with somebody else's text beyond it. A reader with only the flag has
    /// to guess that a wrapped row's content is the whole row, and that guess put a space into a
    /// user's text on every widen (`reflow_drops_the_pad_a_wide_cluster_left_at_the_margin`).
    /// [`line_cells`] is the one reader of this field's meaning; the three consumers of a logical
    /// line — the reflow, the history encoder and the search — all go through it.
    ///
    /// Set by two producers: (1) the autowrap site (the emulator hit the right margin); (2) the
    /// line editor's resize-redraw `CR LF` continuation (`Emulator::in_resize_redraw` — a
    /// premature break that is semantically a soft wrap). Both know the column because both wrap
    /// AT the cursor. Cleared when a row is erased or a line feed ends the line OUTSIDE that
    /// redraw. The second producer is deliberate, not a stray writer: a reflowing
    /// terminal must treat the editor's redraw continuation as soft for it to
    /// collapse on widen, and that `CR LF` is context-only-distinguishable from a
    /// hard newline (it lands mid-row), so the emulator owns that one decision.
    continues: Vec<Option<u16>>,
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
    /// The next [`Image::seq`] [`Self::add_image`] will assign — a monotonic content generation so a
    /// re-transmit that replaces an image id is distinguishable from a re-poll of the same content
    /// (R1404 Stage 5 on-demand transport). Bumped on every insert.
    next_image_seq: u64,
    /// Monotonic count of mutations to everything [`Self::history_bytes`] encodes — the visible cells
    /// AND the scrollback — so a would-be persister can ask "has anything I would encode changed?"
    /// in O(1) instead of encoding the whole scrollback to find out.
    ///
    /// Why the row [`generations`](Self::generations) cannot answer it alone: they cover the VISIBLE
    /// grid only, so `clear_scrollback` (`CSI 3 J`) would erase history without moving any of them,
    /// and a `trim` eviction touches no row either. Why the scrollback alone cannot either: the
    /// encoding includes the visible rows, so a pane that has printed but not yet scrolled has new
    /// content and an unchanged scrollback. Both halves, one counter.
    ///
    /// CONSERVATIVE by construction: it counts mutations, not content changes, so rewriting a cell
    /// with the value it already held bumps it. That direction is the safe one — a stale epoch would
    /// serve stale history on the next restore, an over-eager one only costs an encode that the
    /// byte-compare then discards. It also lives on the SCREEN rather than the emulator, which is what
    /// makes it correct across the alt screen: an alt-screen app writing at full tilt bumps ITS
    /// screen's epoch while the main screen — the one whose history is persisted — stays still.
    content_epoch: u64,
    /// Count of COMPLETE logical lines currently in [`Self::scrollback`] (each ends in a
    /// non-[`wrapped`](Self::wrapped) row), maintained incrementally so [`Self::trim_scrollback`]
    /// can enforce the [`history_limit`](Self::history_limit) on the hot scroll path in O(1). A cached aggregate of the
    /// deque; every scrollback mutation routes through [`Self::push_scrollback`] /
    /// [`Self::trim_scrollback`] / [`Self::clear_scrollback`] so it cannot desync (a debug
    /// assertion in [`Self::reflowed`] re-checks it against a full recount).
    scrollback_logical: usize,
    /// How many complete logical lines this screen has EVER shed into scrollback — monotonic.
    ///
    /// ⚠⚠ The sibling of [`scrollback_logical`](Self::scrollback_logical), and the difference is
    /// the whole point: that one counts what is RETAINED and moves both ways, so an index into it
    /// means a different line after a trim. This one only ever grows, so an absolute line number
    /// keeps its meaning for the life of the screen — which is what lets a consumer say *"I have
    /// had everything up to line N"* and be told honestly how much it MISSED.
    logical_shed: u64,
    /// How many logical lines of scrollback THIS screen retains — tmux's `history-limit`, per
    /// screen because it is a setting the user changes rather than a property of the emulator.
    ///
    /// `0` is a value, not an absence: it means keep no history at all, which is what a user asking
    /// for a pane that remembers nothing has asked for. [`DEFAULT_SCROLLBACK_LINES`] is what a
    /// screen born with nothing configured gets.
    ///
    /// Carried by every DERIVED screen — [`Self::resized`], [`Self::reflowed`] and the alt screen —
    /// rather than re-defaulted, because a screen that forgot it would silently evict a user's
    /// raised history on the next resize. Each of those is a `Screen::new` call whose third argument
    /// the compiler forces the author to name, which is why the limit is a constructor parameter and
    /// not a setter.
    history_limit: usize,
}

impl Screen {
    /// A blank `cols x rows` screen retaining `history_limit` logical lines of scrollback, every row
    /// at generation 0.
    ///
    /// `history_limit` is a parameter rather than a default-then-set because the three internal
    /// callers are exactly the places a derived screen must INHERIT it, and a signature that demands
    /// the value cannot be silently forgotten by a fourth added later. Pass
    /// [`DEFAULT_SCROLLBACK_LINES`] for a screen nobody has configured.
    #[must_use]
    pub fn new(cols: u16, rows: u16, history_limit: usize) -> Self {
        let count = cols as usize * rows as usize;
        Self {
            cols,
            rows,
            cells: vec![Cell::blank(); count],
            cursor: Cursor::default(),
            kind: ScreenKind::Main,
            generations: vec![0; rows as usize],
            scrollback: VecDeque::new(),
            continues: vec![None; rows as usize],
            marks: vec![None; rows as usize],
            images: Vec::new(),
            next_image_seq: 0,
            scrollback_logical: 0,
            logical_shed: 0,
            content_epoch: 0,
            history_limit,
        }
    }

    /// How many logical lines of scrollback this screen retains — see [`Self::new`].
    #[must_use]
    pub fn history_limit(&self) -> usize {
        self.history_limit
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
        // Stamp a fresh content generation so a re-transmit (even one that keeps the same id) is
        // distinguishable from a re-poll — the on-demand client keys its cache on `(id, seq)`.
        let mut image = image;
        image.seq = self.next_image_seq;
        self.next_image_seq = self.next_image_seq.wrapping_add(1);
        // An image is CONTENT the history encodes, so every image mutation counts toward the epoch —
        // without this a pane whose only change was an image would read as idle and never be re-saved.
        self.touch_scrollback();
        if let Some(slot) = self.images.iter_mut().find(|i| i.id == image.id) {
            *slot = image;
            return;
        }
        if self.images.len() >= IMAGE_CAP {
            self.images.remove(0);
        }
        self.images.push(image);
    }

    /// Drop every inline image — the screen-clear / alt-screen lifecycle (Stage 1), and the Kitty
    /// delete-all (`a=d, d=a`, Stage 4).
    pub(crate) fn clear_images(&mut self) {
        if !self.images.is_empty() {
            self.touch_scrollback();
        }
        self.images.clear();
    }

    /// Drop the inline image with [`Image::id`] `id` — the Kitty delete-by-id (`a=d, d=i, i=<id>`,
    /// Stage 4). A no-op when no image carries that id.
    pub(crate) fn delete_image(&mut self, id: u32) {
        let before = self.images.len();
        self.images.retain(|img| img.id != id);
        if self.images.len() != before {
            self.touch_scrollback();
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
    ///
    /// The QUESTION a display asks — does this row's line run on? A reader that has to know how
    /// much of the row belongs to that line wants the crate-internal `continues`, because the
    /// answer is not always "all of it".
    #[must_use]
    pub fn wrapped(&self, row: u16) -> bool {
        self.continues(row).is_some()
    }

    /// Row `row`'s soft-wrap continuation: `Some(n)` when its logical line runs onto row `row + 1`
    /// having put `n` cells on this one, `None` when the line ends here (or the row is out of
    /// bounds). The fact [`Self::wrapped`] answers a `bool` from; the private `line_cells` is what
    /// turns it into the row's share of its logical line.
    ///
    /// **Public because a DISPLAY that re-wraps needs the count and not the flag.** A client
    /// narrower than the pane it is watching rebuilds the logical lines and cuts them to its own
    /// width, and it cannot do that from a `bool`: the columns from `n` are LAYOUT — the pad a wide
    /// cluster leaves when it will not fit at the margin — and joining them into the line is the
    /// defect `line_cells`'s own doc records. [`Self::wrapped`] stays as the question a display
    /// asks when it only needs to know THAT the line runs on, and it is derived from this, so the
    /// two cannot disagree.
    #[must_use]
    pub fn continues(&self, row: u16) -> Option<u16> {
        self.continues.get(row as usize).copied().flatten()
    }

    /// Scrolled-off line `index`'s soft-wrap continuation (0 = oldest) — [`Self::continues`] for
    /// the history rows, as [`Self::scrollback_mark`] is [`Self::mark`] for them.
    ///
    /// A scrolled-back view projects history rows beside visible ones, so a client re-wrapping that
    /// view needs the same fact about both halves; without this one, the rows above the live region
    /// would join into the wrong lines.
    #[must_use]
    pub fn scrollback_continues(&self, index: usize) -> Option<u16> {
        self.scrollback.get(index).and_then(|line| line.continues)
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

    /// A row's share of its LOGICAL line, as text — [`Self::row_text`] for a row a line ends on,
    /// and only the cells that belong to the line for a row it soft-wraps out of.
    ///
    /// The reader for anything that wants a wrapped line's CONTENT rather than its position: join
    /// consecutive shares with nothing between them and what comes back is what the child printed.
    /// [`Self::row_text`] cannot be joined that way — it trims a continuing row's trailing blanks,
    /// which are interior to the line, and it keeps the pad a wide cluster left at the margin,
    /// which is not in the line at all. Both halves have cost this project a defect; `line_cells`
    /// is the one place either is decided, and this is its public spelling for a caller outside
    /// the search.
    ///
    /// Empty for a row out of bounds, as [`Self::row_text`] is.
    #[must_use]
    pub fn row_share_text(&self, row: u16) -> String {
        let mut text = String::new();
        for cell in self.row_share(row) {
            text.push_str(&cell.cluster);
        }
        text
    }

    /// A row's share of its logical line, as CELLS — `line_cells` over one live row, and the one
    /// thing [`Self::row_share_text`], [`Self::row_share_len`] and the grid projection all read.
    ///
    /// Borrowed rather than cloned, and that is not a micro-optimisation: this is on the per-frame
    /// path now that a display asks it of every row, and [`Self::row_cells`] clones a heap cluster
    /// per cell. R344 shipped a 6x per-keystroke regression of exactly that shape.
    #[must_use]
    fn row_share(&self, row: u16) -> &[Cell] {
        let Some(start) = self.index(0, row) else {
            return &[];
        };
        let cells = &self.cells[start..start + self.cols as usize];
        line_cells(cells, self.continues(row))
    }

    /// How many of row `row`'s cells belong to its logical line — the length of the row's share
    /// (the private `row_share`), and `0` out of bounds.
    ///
    /// The count a display re-wrapping this pane needs for every row: for a row its line runs off,
    /// it is [`Self::continues`]'s column; for a row the line ENDS on, it is where the text stops
    /// and the grid's padding starts. Both answers come from `line_cells`, so a client cutting
    /// lines at these positions is cutting them where the reflow, the durable history and the
    /// search all agree they end.
    #[must_use]
    pub fn row_share_len(&self, row: u16) -> u16 {
        u16::try_from(self.row_share(row).len()).unwrap_or(u16::MAX)
    }

    /// How many cells row `row` actually HOLDS — up to its last non-blank, ignoring whatever
    /// continuation flag it currently carries.
    ///
    /// ⚠ The difference from [`row_share_len`](Self::row_share_len) is the flag: that one reports
    /// the row's share of its line AS CURRENTLY RECORDED, and this one reports what is there. It is
    /// what a writer needs when it is about to RECORD a continuation, because the number it must
    /// store is how many cells the line put on this row — and the cursor is not that number once a
    /// CARRIAGE RETURN has moved it back over content that is still present.
    #[must_use]
    pub fn row_content_len(&self, row: u16) -> u16 {
        u16::try_from(line_cells(&self.row_cells(row), None).len()).unwrap_or(u16::MAX)
    }

    /// [`Self::row_share_len`] for scrolled-off line `index` (0 = oldest) — the pair to
    /// [`Self::scrollback_continues`], as [`Self::scrollback_mark`] pairs with [`Self::mark`].
    ///
    /// Through `line_cells` and NOT the stored length, because a history line is not stored
    /// pre-trimmed to its share: the history encoder applies the same rule to the same cells, and
    /// a second answer here would be a client cutting lines where nothing else does.
    #[must_use]
    pub fn scrollback_share_len(&self, index: usize) -> u16 {
        self.scrollback.get(index).map_or(0, |line| {
            u16::try_from(line_cells(&line.cells, line.continues).len()).unwrap_or(u16::MAX)
        })
    }

    /// The COMPLETED logical lines this screen has produced after absolute line `cursor`, and how
    /// many were lost before the reader got there.
    ///
    /// # ⚠⚠ Why a consumer of a pane's output needs this and not the grid
    ///
    /// *"What has this pane produced since I last looked?"* has been answered here by comparing
    /// ROWS — their damage generations, then their text — and every such answer is a claim about a
    /// RENDERING rather than about output. A row is not a unit the child produced: it is where the
    /// terminal happened to break a line at the width it happened to have. So
    ///
    /// * a **RESIZE** re-wraps every row and re-numbers them, and a row-keyed reader either
    ///   re-delivers the screen or loses its place;
    /// * a **REPAINT** (a palette change, a redraw) changes no content at all, and a
    ///   generation-keyed reader calls it output;
    /// * **SCROLLING** silently drops what a row-keyed reader never came back for.
    ///
    /// A LOGICAL line is the unit the child actually produced, and reflow is defined as preserving
    /// it. Numbering those lines from the screen's birth gives an ADDRESS: line 4 is the same line
    /// after any number of resizes, so a cursor means *"I have had everything up to here"* and can
    /// be honoured EXACTLY ONCE even if the rows carrying it are repainted a hundred times.
    ///
    /// # What it answers, and what it admits
    ///
    /// Lines are yielded oldest-first in [`LinesSince::lines`], joined across their soft-wrapped
    /// rows through [`row_share_text`](Self::row_share_text) — so a line the terminal wrapped comes
    /// back as the child wrote it, at any width.
    ///
    /// ⚠ **Only COMPLETE lines.** The line the cursor is on is still being written and is never
    /// yielded — a consumer that acted on half a line would act on something the child had not
    /// finished saying.
    ///
    /// ⚠ **There is deliberately no second spelling of *"how many lines has it produced"***. It is
    /// [`LinesSince::next`], and a `lines_produced()` convenience existed here briefly with NO
    /// caller — publishing a second way to ask one question is how the two answers come to differ.
    /// Pass `u64::MAX` to mark without taking anything.
    ///
    /// ⚠⚠ **AND A LOSS IS REPORTED, NOT HIDDEN.** Scrollback is bounded, so a reader that stays
    /// away longer than the history is deep cannot be given what it missed. [`LinesSince::lost`] is
    /// how many lines that was. **The alternative is the one this project keeps paying for**: a
    /// silent gap looks exactly like a quiet source, and a consumer cannot tell *nothing happened*
    /// from *I was not fast enough*.
    #[must_use]
    pub fn lines_since(&self, cursor: u64) -> LinesSince {
        // The oldest line still retained: everything shed, minus what scrollback still holds.
        let oldest = self.logical_shed - self.scrollback_logical as u64;
        let lost = oldest.saturating_sub(cursor);
        let from = cursor.max(oldest);

        let mut lines = Vec::new();
        // The scrollback half: logical lines rebuilt from their soft-wrapped rows, counted so the
        // absolute index of each is known without storing one per row.
        let mut at = oldest;
        let mut joined = String::new();
        for index in 0..self.scrollback.len() {
            joined.push_str(&cells_text(line_cells(
                &self.scrollback[index].cells,
                self.scrollback[index].continues,
            )));
            if self.scrollback_continues(index).is_none() {
                if at >= from {
                    lines.push(std::mem::take(&mut joined).trim_end().to_string());
                } else {
                    joined.clear();
                }
                at += 1;
            }
        }

        // The visible half: rows ABOVE the cursor's, which the child has moved past. The cursor's
        // own line is unfinished, and a run that continues into it is unfinished with it.
        joined.clear();
        for row in 0..self.cursor.row.min(self.rows) {
            joined.push_str(&self.row_share_text(row));
            if self.continues(row).is_none() {
                if at >= from {
                    lines.push(std::mem::take(&mut joined).trim_end().to_string());
                } else {
                    joined.clear();
                }
                at += 1;
            }
        }

        joined.push_str(&self.row_share_text(self.cursor.row));
        LinesSince {
            lines,
            next: at,
            lost,
            partial: joined.trim_end().to_string(),
        }
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

    /// The pane's retained output encoded as REPLAYABLE terminal bytes — the durable form of this
    /// screen's history, bounded to its last `limit` LOGICAL lines (`0` encodes nothing).
    ///
    /// Same axis as [`Self::full_text`] and [`Self::find`] — scrollback then the visible rows — so
    /// what a restore brings back is exactly what a search can find. Feeding the result to a fresh
    /// [`Emulator`](crate::Emulator) of the same width reconstructs these cells, their styles,
    /// their OSC-8 links, their prompt marks and their soft-wrap structure. The crate-internal
    /// `history` module documents why terminal bytes are the durable form and what alphabet the
    /// encoder generates (SGR, OSC 8, OSC 133, clusters and `CR LF` — nothing else).
    ///
    /// The visible region contributes only up to its last non-blank row: the empty grid below a
    /// short screen is padding, and encoding it would restore a screenful of blank lines above the
    /// new shell's prompt.
    #[must_use]
    pub fn history_bytes(&self, limits: HistoryLimits) -> Vec<u8> {
        let cols = self.cols as usize;
        let blank = Cell::blank();
        // Per visible row, the images anchored on it with their columns, in transmit order — the
        // encoder places each one where its anchor cell is written.
        let mut anchored: Vec<Vec<(u16, &Image)>> = vec![Vec::new(); self.rows as usize];
        if limits.image_bytes > 0 {
            for image in &self.images {
                if let Some(slot) = anchored.get_mut(image.anchor.1 as usize) {
                    slot.push((image.anchor.0, image));
                }
            }
            for slot in &mut anchored {
                // By column, so the encoder's single left-to-right walk can place them without
                // seeking backwards; transmit order is preserved within a column by the stable sort.
                slot.sort_by_key(|(col, _)| *col);
            }
        }
        let has_content = |row: usize| {
            self.cells[row * cols..(row + 1) * cols]
                .iter()
                .any(|cell| *cell != blank)
                // A row carrying an IMAGE is not empty, whatever its cells say: an image displayed
                // below the last line of text sits on a row of blanks, and trimming that row away
                // would discard the image with it.
                || !anchored[row].is_empty()
        };
        let visible_end = (0..self.rows as usize)
            .rev()
            .find(|row| has_content(*row))
            .map_or(0, |row| row + 1);
        let rows: Vec<HistoryRow<'_>> = self
            .scrollback
            .iter()
            .map(|line| HistoryRow {
                cells: &line.cells,
                continues: line.continues,
                mark: line.mark,
                // Scrollback carries no images: an image scrolled off the top is evicted, never
                // retained (the Stage-1 lifecycle this encoder inherits).
                images: &[],
            })
            .chain((0..visible_end).map(|row| HistoryRow {
                cells: &self.cells[row * cols..(row + 1) * cols],
                continues: self.continues[row],
                mark: self.marks[row],
                images: &anchored[row],
            }))
            .collect();
        crate::history::encode(&rows, limits)
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

        let text = |range: std::ops::Range<usize>| self.logical_text_in(range);

        let exit_status = match d {
            Some(i) => match mark_at(i) {
                Some(PromptMark::CommandEnd(status)) => status,
                _ => None,
            },
            None => None,
        };
        Some(LastCommand {
            command: a.map(|a| text(a..c)).unwrap_or_default(),
            output: text(c..d.map_or(total, |d| d + 1)),
            exit_status,
            running: d.is_none(),
        })
    }

    /// The RETAINED ROW (from the oldest, `0`) of every OSC 133 prompt-start
    /// ([`PromptMark::Prompt`]) mark — oldest first, across scrollback then the visible grid.
    /// These are the jump-to-prompt targets: a display client's scroll `offset_y` IS the view's top
    /// retained row, so jumping the view to prompt `L` is `scroll_to(L)`. Empty without shell
    /// integration. Bounded (scrollback cap + rows).
    ///
    /// A mark rides its logical line's FIRST row (which is what a reflow re-attaches it
    /// to), so these are line STARTS on the row axis — the same values [`FindMatch::line`] reports,
    /// and the reason a search answer and a prompt jump are interchangeable scroll targets.
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

    /// Find every literal occurrence of `needle` in the pane's retained output — scrollback first,
    /// then the visible grid — as [`FindMatch`]es in the logical line + cell-column coordinate.
    ///
    /// This is the find-in-scrollback SSOT, and it lives HERE, beside the cells, for two reasons a
    /// client-side search cannot have: the columns are only derivable from the CELLS (a wide cluster
    /// is one cluster but two columns, so a byte offset into the text is not a column), and the
    /// answer is tiny where the haystack is not — a display client in another process asks for
    /// matches instead of pulling a whole scrollback over the socket on every keystroke.
    ///
    /// Semantics, all deliberate and all observable:
    /// - **Literal**, not a pattern — a needle is what the user typed. (A regex mode would be a
    ///   distinct query, not a flag that silently reinterprets the same string.)
    /// - **ASCII case-INSENSITIVE.** Folding only ASCII is what keeps the column arithmetic exact:
    ///   `to_ascii_lowercase` preserves every byte offset, whereas full Unicode folding can change a
    ///   string's length (`İ` lowercases to two chars) and would slide every column after it. Scripts
    ///   without case (한글, 漢字, かな) are unaffected either way.
    /// - **Per LINE.** A match never spans a line break, so a needle with `\n` finds nothing.
    /// - **Non-overlapping**: the scan resumes after each match, so `aa` occurs once in `aaa`.
    /// - **Trailing blanks are unsearchable.** They are the grid's padding out to `cols`, not
    ///   content — otherwise a needle of spaces would match every row. [`Screen::row_text`] trims for
    ///   the same reason.
    /// - An EMPTY needle matches nothing (a needle is a thing to look for, not a position).
    ///
    /// Bounded by [`FIND_MATCH_CAP`]; [`FindResult::truncated`] reports when the cap bit.
    #[must_use]
    pub fn find(&self, needle: &str) -> FindResult {
        if needle.is_empty() {
            return FindResult::default();
        }
        let needle = needle.to_ascii_lowercase();
        // Two scratch buffers for the whole search, reused line to line: a scrollback-deep scan
        // would otherwise allocate twice per line to answer one keystroke.
        let mut text = String::new();
        let mut starts = Vec::new();
        self.scan_logical(|line_cells, line, out| {
            find_in_line(line_cells, &needle, line, &mut text, &mut starts, out)
        })
    }

    /// Every non-overlapping match of the REGULAR EXPRESSION `pattern` in the pane's retained
    /// output, in the same coordinates [`Self::find`] answers in — or the engine's own explanation
    /// of why the pattern was rejected.
    ///
    /// ## A distinct search, not a mode on the literal one
    ///
    /// A needle and a pattern are different LANGUAGES, and the same string means different things
    /// in each: `a.b` is three literal characters to [`Self::find`] and "a, anything, b" here.
    /// So this is its own entry with its own address on the wire — a flag that reinterpreted a
    /// needle in place would silently change what an already-typed search means.
    ///
    /// The case rule differs for the same reason, and deliberately: [`Self::find`] is ASCII
    /// case-INSENSITIVE because a literal search is a convenience, while a pattern is
    /// case-SENSITIVE because the language already has `(?i)` and folding underneath it would
    /// overrule what the caller wrote.
    ///
    /// Zero-width matches (`x*` against a line with no `x`) are not reported: they cover no cells,
    /// so there is nothing to highlight and nothing a coordinate could point at. The scan still
    /// advances past them, so such a pattern terminates rather than looping.
    ///
    /// Bounded twice over. Matching is linear in the input by construction — the engine admits no
    /// backtracking, which is what makes a caller-supplied pattern safe to run on the dispatch
    /// thread at all — and COMPILATION is capped at [`REGEX_SIZE_LIMIT`], so a pathological pattern
    /// is refused rather than spending the interactive path's time building an enormous program.
    /// Match count is capped by [`FIND_MATCH_CAP`], reported as
    /// [`truncated`](FindResult::truncated), exactly as for the literal search.
    ///
    /// # Errors
    ///
    /// [`BadPattern`] when `pattern` is not a valid regular expression or exceeds the size limit,
    /// carrying the engine's message so a caller can show WHERE it went wrong. An EMPTY pattern is
    /// not an error — it matches nothing, mirroring an empty needle.
    pub fn find_regex(&self, pattern: &str) -> Result<FindResult, BadPattern> {
        if pattern.is_empty() {
            return Ok(FindResult::default());
        }
        let regex = regex::RegexBuilder::new(pattern)
            .size_limit(REGEX_SIZE_LIMIT)
            .build()
            .map_err(|error| BadPattern(error.to_string()))?;
        let mut text = String::new();
        let mut starts = Vec::new();
        Ok(self.scan_logical(|line_cells, line, out| {
            regex_in_line(line_cells, &regex, line, &mut text, &mut starts, out)
        }))
    }

    /// Retained row `index`'s cells and its soft-wrap continuation — scrollback first (`0` = the
    /// oldest), then the visible grid. `None` past the last retained row.
    ///
    /// THE one place the two halves are indexed as one axis. Three readers needed it and each had
    /// its own copy of the `if index < scrollback.len()` split; a fourth would have written a
    /// fourth. The visible half is borrowed straight out of the row-major cell buffer, so walking
    /// the whole retained region allocates nothing.
    fn retained_row(&self, index: usize) -> Option<(&[Cell], Option<u16>)> {
        let sb_len = self.scrollback.len();
        if index < sb_len {
            let history = &self.scrollback[index];
            return Some((history.cells.as_slice(), history.continues));
        }
        let row = index - sb_len;
        if row >= self.rows as usize {
            return None;
        }
        let cols = self.cols as usize;
        Some((
            &self.cells[row * cols..(row + 1) * cols],
            self.continues[row],
        ))
    }

    /// The retained rows in `range` as TEXT, soft wraps joined and hard breaks kept as `"\n"`,
    /// with trailing empty lines dropped.
    ///
    /// The text half of [`Self::scan_logical`]'s traversal, and it agrees with it line for line —
    /// which is what makes a search answer and a command slice describe the same lines. A line
    /// still open at the range's end is closed there: the range is what the caller asked about, and
    /// the rows past it belong to somebody else's slice.
    fn logical_text_in(&self, range: std::ops::Range<usize>) -> String {
        let mut lines: Vec<String> = Vec::new();
        let mut joined: Vec<Cell> = Vec::new();
        for index in range.clone() {
            let Some((cells, continues)) = self.retained_row(index) else {
                break;
            };
            joined.extend_from_slice(line_cells(cells, continues));
            if continues.is_some() && index + 1 < range.end {
                continue;
            }
            lines.push(cells_text(&joined));
            joined.clear();
        }
        while lines.last().is_some_and(String::is_empty) {
            lines.pop();
        }
        lines.join("\n")
    }

    /// Run `scan` over every retained LOGICAL line — scrollback first, then the visible grid, as
    /// ONE stream — collecting its matches and, for each line that produced one, its text.
    ///
    /// ## A logical line, not a row: the traversal IS the fix
    ///
    /// A pane holds ROWS; a person reads LINES. A line that ran past the right margin occupies
    /// several consecutive rows, and this walk joins them — through [`line_cells`], so each row
    /// contributes exactly its own share — before the search ever sees them. Scanning row by row
    /// (which every search here did until R344) cannot find the word a person is looking straight
    /// at: on a 20-column pane, `abcdefghijklmnopqrstuvwxyz` is two rows and the needle is in
    /// neither of them. It cost `sprag find`, `find_in_pane`, `regex_in_pane` and — worst —
    /// `wait-for-output`, which simply never fired for a needle that happened to wrap.
    ///
    /// The join spans the scrollback→visible boundary for the same reason [`Self::reflowed`] treats
    /// the two as one stream: a line half scrolled off is still one line. The last retained row may
    /// carry a continuation (a line running past what is kept); it closes the line anyway, exactly
    /// as the history encoder does — the rest is not ours to search.
    ///
    /// ## Two coordinates, because there are two questions
    ///
    /// `scan` is handed the JOINED cells plus the [`RowShare`]s that say which row each of them
    /// came from, so a match found in line coordinates goes back onto the grid ([`grid_span`]) as
    /// the rows it really covers. `line` — the index it reports — is the retained row the LOGICAL
    /// line begins on, which is the axis [`Self::prompt_positions`] reports and a client's scroll
    /// `offset_y` already speaks.
    ///
    /// `scan` returns `false` when it hit the match cap, which ends the sweep and marks the result
    /// truncated. The line text is derived AFRESH from the cells rather than taken from `scan`'s
    /// scratch buffer, which the literal search lowercases in place.
    fn scan_logical(
        &self,
        mut scan: impl FnMut(LogicalLine<'_>, usize, &mut Vec<FindMatch>) -> bool,
    ) -> FindResult {
        let mut result = FindResult::default();
        let retained = self.scrollback.len() + self.rows as usize;
        // The line being walked: one borrowed slice per row it occupies, and the row it begins
        // on. Reused across lines — a scrollback-deep search must not allocate per line, and it
        // never copies a cell (see [`RowShare`]).
        let mut shares: Vec<RowShare<'_>> = Vec::new();
        let mut width = 0usize;
        let mut line = 0usize;
        for row in 0..retained {
            let Some((cells, continues)) = self.retained_row(row) else {
                break;
            };
            if shares.is_empty() {
                line = row;
            }
            let mine = line_cells(cells, continues);
            shares.push(RowShare {
                row,
                start: width,
                cells: mine,
            });
            width += mine.len();
            // A continuation keeps the line open — unless there is no next row to continue onto.
            if continues.is_some() && row + 1 < retained {
                continue;
            }
            let before = result.matches.len();
            let within_cap = scan(LogicalLine(&shares), line, &mut result.matches);
            if result.matches.len() > before {
                result.lines.push(FindLine {
                    line,
                    text: shares_text(&shares),
                });
            }
            shares.clear();
            width = 0;
            if !within_cap {
                result.truncated = true;
                return result;
            }
        }
        result
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
                                text: cell.cluster.to_string(),
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

    /// Record (or clear) row `row`'s soft-wrap continuation. `Some(upto)` says the row's logical
    /// line runs onto the next row having put `upto` cells here; `None` says the line ends on this
    /// row. The VT backend sets it AT THE WRAP, where the column is the cursor's, and clears it
    /// when a row is erased or a hard line feed ends the line. Out-of-bounds rows are ignored.
    ///
    /// The column is a parameter rather than derived from `cols` because the two disagree exactly
    /// where it matters: a wide cluster that will not fit wraps a column early, and a DECLRMM
    /// region ends before the screen does. See the [`continues`](Self::continues) field.
    pub(crate) fn set_wrapped_at(&mut self, row: u16, upto: Option<u16>) {
        if let Some(slot) = self.continues.get_mut(row as usize) {
            *slot = upto;
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

    /// Mark EVERY row dirty at `generation` — a whole-screen APPEARANCE change that touches no
    /// cells. A colour-palette change (`OSC 4 / 10 / 11 / 12`) re-colours existing cells: they keep
    /// their symbolic [`Color`], but resolve differently against the new palette. Since a
    /// generation-gated painter (pinion's `TextGrid` re-rasterizes only rows whose damage stamp
    /// advanced) would otherwise keep the stale colours, bumping every row's generation forces the
    /// re-colour to reach the display. No cells change, so this is the damage peer of an `OSC 4` set.
    pub(crate) fn mark_all_dirty(&mut self, generation: u64) {
        for g in &mut self.generations {
            *g = generation;
        }
    }

    /// Write a cell and bump the owning row's damage generation.
    ///
    /// ⚠ A write AT OR PAST the column this row wrapped at EXTENDS its share of the line, because
    /// the cell is now content whatever put it there. Without that, a stale wrap column HIDES a
    /// character that is on the screen: a row that wrapped at column 3 and is then written at
    /// column 4 by direct cursor addressing renders `"ab世Z"` and answered NOTHING for a search
    /// for `Z` — the exact blindness R344 exists to remove, re-introduced one layer down by its
    /// own fix. Measured before this line existed.
    ///
    /// EXTENDED rather than cleared: the row's line does still continue onto the next (that is
    /// what the flag says and it is still true), and only how much of THIS row belongs to it has
    /// changed. Clearing would split one logical line into two on a plain overwrite. The ordinary
    /// autowrap path never reaches this — it sets the column and immediately moves to the next row
    /// — so this fires only for a writer that goes back.
    pub(crate) fn set_cell(&mut self, col: u16, row: u16, cell: Cell, generation: u64) {
        if let Some(i) = self.index(col, row) {
            self.cells[i] = cell;
            self.stamp_row(row, generation);
            if let Some(slot) = self.continues.get_mut(row as usize)
                && slot.is_some_and(|upto| col >= upto)
            {
                *slot = Some(col.saturating_add(1));
            }
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
            self.stamp_row(row, generation);
            // An erased row no longer continues a logical line, and its shell-integration mark
            // (if any) goes with the content that was cleared.
            self.continues[row as usize] = None;
            self.marks[row as usize] = None;
            // An inline image anchored on this row goes with the cleared content (R1404 Stage 3):
            // erase-in-display (ED, which clear_row's the affected rows) drops it, no ghost left.
            // During a scroll this is a no-op — images shift before the vacated rows are cleared.
            self.images.retain(|img| img.anchor.1 != row);
        }
    }

    /// Insert `n` blank cells at `(col, row)`, shifting the cells from `col` rightward by `n`
    /// (ICH — INSERT CHARACTER). Cells pushed past `right` — the RIGHT MARGIN, which is the last
    /// column unless DECLRMM has narrowed the region — fall off; the opened gap `[col, col+n)`
    /// becomes blank, and the columns beyond `right` are untouched. Row-local (no top/bottom
    /// margin interaction). Bumps the row's damage generation; a shift breaks the row's soft-wrap
    /// continuation (its tail changed), so the wrap flag is cleared.
    pub(crate) fn insert_cells(&mut self, col: u16, row: u16, n: u16, right: u16, generation: u64) {
        if row >= self.rows || col >= self.cols || n == 0 {
            return;
        }
        let base = row as usize * self.cols as usize;
        let col = col as usize;
        let end = (right.min(self.cols - 1) as usize) + 1; // one past the right margin
        if col >= end {
            return; // the cursor sits right of the margin: nothing to shift
        }
        let n = (n as usize).min(end - col);
        // Shift right: move [col, end-n) to [col+n, end), walking from the right so a source is
        // read before it is overwritten.
        for dst in (col + n..end).rev() {
            self.cells[base + dst] = self.cells[base + dst - n].clone();
        }
        for cell in &mut self.cells[base + col..base + col + n] {
            *cell = Cell::blank();
        }
        self.stamp_row(row, generation);
        self.continues[row as usize] = None;
    }

    /// Delete `n` cells at `(col, row)`, shifting the cells from `col+n` leftward to `col` and
    /// blanking the `n` cells vacated at the RIGHT MARGIN `right` (DCH — DELETE CHARACTER).
    /// Row-local, the inverse of [`Self::insert_cells`], and like it it leaves the columns beyond
    /// `right` alone. Bumps the row's generation and clears its wrap flag.
    pub(crate) fn delete_cells(&mut self, col: u16, row: u16, n: u16, right: u16, generation: u64) {
        if row >= self.rows || col >= self.cols || n == 0 {
            return;
        }
        let base = row as usize * self.cols as usize;
        let col = col as usize;
        let end = (right.min(self.cols - 1) as usize) + 1; // one past the right margin
        if col >= end {
            return; // the cursor sits right of the margin: nothing to shift
        }
        let n = (n as usize).min(end - col);
        // Shift left: move [col+n, end) to [col, end-n), walking from the left.
        for dst in col..end - n {
            self.cells[base + dst] = self.cells[base + dst + n].clone();
        }
        for cell in &mut self.cells[base + end - n..base + end] {
            *cell = Cell::blank();
        }
        self.stamp_row(row, generation);
        self.continues[row as usize] = None;
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
        self.stamp_row(row, generation);
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
        // The retention limit is this screen's own: a resize re-lays-out content, it does not
        // re-decide how much of it the user asked to keep.
        let mut next = Screen::new(cols, rows, self.history_limit);
        let copy_cols = cols.min(self.cols);
        let copy_rows = rows.min(self.rows);
        for r in 0..copy_rows {
            for c in 0..copy_cols {
                if let (Some(src), Some(dst_i)) = (self.cell(c, r), next.index(c, r)) {
                    next.cells[dst_i] = src.clone();
                }
            }
            next.generations[r as usize] = self.generations[r as usize];
            // The wrap column is CLAMPED to the new width: this fallback truncates a row's cells
            // at `copy_cols`, so a continuation that named a column past the new margin would
            // claim content the copy did not bring.
            next.continues[r as usize] = self.continues(r).map(|upto| upto.min(copy_cols));
            next.marks[r as usize] = self.marks[r as usize];
        }
        next.cursor = self.cursor;
        next.kind = self.kind;
        // This is the reflow FALLBACK (alt screen / degenerate size): scrollback carries across
        // verbatim, NOT rewrapped. The main-screen path is [`Self::reflowed`], which DOES rewrap
        // history to the new width; an alt-screen app owns its own layout, so it stays verbatim.
        next.scrollback = self.scrollback.clone();
        next.scrollback_logical = self.scrollback_logical; // same lines -> same logical count
        // ⚠⚠ AND THE MONOTONIC TOTAL CARRIES, which is the invariant a reader's cursor rests on: a
        // resize re-wraps ROWS and cannot create or destroy a LOGICAL line, so re-deriving this
        // would double-count every line on every resize and silently re-deliver the lot.
        next.logical_shed = self.logical_shed;
        // Inline images (Kitty / Sixel) carry across a resize verbatim — a plain
        // resize must NOT drop them. This is the alt-screen / degenerate fallback: the app owns its
        // layout, so the anchor is not re-mapped here (the main-screen rewrap in [`Self::reflowed`]
        // does that). `next_image_seq` carries so a post-resize re-transmit stays monotonic.
        next.images = self.images.clone();
        next.next_image_seq = self.next_image_seq;
        // The epoch CARRIES AND ADVANCES past this screen's: a resize re-lays-out the content, so a
        // persister holding an older reading must re-encode. Carrying it unchanged would let a resize
        // pass for "nothing happened"; resetting to 0 would make the new screen's epoch read as OLDER
        // than an observation already taken, which is the one direction that serves stale history.
        next.content_epoch = self.content_epoch.wrapping_add(1);
        next
    }

    /// A copy reflowed to `cols x rows`: the MAIN screen's LOGICAL lines — the SCROLLBACK
    /// history AND the visible grid, joined into ONE stream by the soft-wrap flag
    /// ([`Self::wrapped`]) — are re-broken at the new width, so a resize rewraps cleanly
    /// instead of leaving a live shell's per-width prompt redraws stacked up (the verbatim
    /// [`Self::resized`] bug) OR width-stale history frozen at its old margin. Treating
    /// scrollback + visible as one stream yields two behaviours a verbatim carry cannot:
    /// narrowing rewraps the scrolled-off history to the new width, and GROWING the height
    /// reclaims scrolled-off lines back into the visible area (the bottom-anchored materialize
    /// in Pass 3 pulls history down) — matching xterm/kitty, where tmux reflows neither. The
    /// alternate screen and degenerate sizes fall back to [`Self::resized`] (a fullscreen app
    /// owns its own layout; scrollback stays verbatim there). The cursor tracks its LOGICAL line
    /// across the rewrap but anchors to that line's FIRST physical row (its column preserved),
    /// so a live line editor's resize redraw overwrites in place rather than stacking — see the
    /// cursor-anchor note in Pass 3. Wide clusters never split across the margin; overflow above
    /// the visible window materializes as the new scrollback (as styled cells, wrap + mark
    /// preserved). Inline images track their ANCHOR CELL across the rewrap and are evicted if it
    /// reflows above the window. `gen` is a fresh damage stamp for every (re-laid-out) visible row.
    pub(crate) fn reflowed(&self, cols: u16, rows: u16, generation: u64) -> Screen {
        if self.kind != ScreenKind::Main || cols == 0 || rows == 0 {
            return self.resized(cols, rows);
        }
        // Pass 1 — reconstruct logical lines from glyph cells (trailers dropped),
        // joining soft-wrapped rows; trim trailing blanks at a hard line end. The
        // SCROLLBACK history leads the stream, then the visible grid, so a scrolled-off
        // line rewraps to the new width and a line spanning the boundary rejoins across it.
        // Track the cursor's (logical line, glyph offset) — the cursor is in the visible part.
        let mut lines: Vec<Vec<Cell>> = Vec::new();
        // Parallel to `lines`: each logical line's shell-integration mark (its FIRST physical
        // row's), so the mark survives the rewrap by re-attaching to the re-broken line's head.
        let mut line_marks: Vec<Option<PromptMark>> = Vec::new();
        let mut cur: Vec<Cell> = Vec::new();
        let mut cur_mark: Option<PromptMark> = None;
        let (cur_col, cur_row) = (self.cursor.col, self.cursor.row);
        let (mut cursor_line, mut cursor_off, mut cursor_found) = (0usize, 0usize, false);
        // Per-image anchor tracking across the rewrap, parallel to `self.images` — the same idea as
        // the cursor: Pass 1 records the anchor cell's (logical line, glyph offset), Pass 2 maps it
        // to the new (col, physical row), Pass 3 keeps it if that row stays visible and EVICTS it if
        // the cell reflowed ABOVE the window (no scrollback images — the Stage-3 bound, matching a
        // scrolled-off image). `images_on_row` lets the Pass-1 cell scan check only the images on the
        // current row; an anchor row outside the visible grid is unrepresented, so that image's
        // `anchor_pos` stays `None` and it is evicted. Images only ever anchor in the visible grid
        // (a scroll evicts one that leaves the top), so scanning the visible rows alone suffices.
        let mut anchor_pos: Vec<Option<(usize, usize)>> = vec![None; self.images.len()];
        let mut anchor_phys: Vec<Option<(u16, usize)>> = vec![None; self.images.len()];
        let mut images_on_row: Vec<Vec<usize>> = vec![Vec::new(); self.rows as usize];
        for (i, img) in self.images.iter().enumerate() {
            if img.anchor.1 < self.rows {
                images_on_row[img.anchor.1 as usize].push(i);
            }
        }
        // Scrollback rows first (oldest -> newest), no cursor. A wrapped last scrollback line
        // leaves `cur` open so the first visible row continues its logical line.
        for line in &self.scrollback {
            if cur.is_empty() {
                cur_mark = line.mark;
            }
            // The row's share of its logical line — [`line_cells`] is what knows that a CONTINUING
            // row keeps its printed blanks and drops the pad an early wrap left.
            for cell in line_cells(&line.cells, line.continues) {
                if cell.width == Width::Trailer {
                    continue; // regenerated when the wide head is re-placed
                }
                cur.push(cell.clone());
            }
            if line.continues.is_none() {
                lines.push(std::mem::take(&mut cur));
                line_marks.push(cur_mark);
            }
        }
        for r in 0..self.rows {
            // A logical line's mark is its FIRST physical row's mark; when `cur` is empty this
            // row begins a new logical line, so capture its mark (continuation rows keep it).
            if cur.is_empty() {
                cur_mark = self.marks[r as usize];
            }
            let row_cells = self.row_cells(r);
            // This row's share of its logical line: the cells past it are the grid's padding at a
            // hard end, or the pad / another DECLRMM column at a soft one. The column walk below
            // still visits EVERY column, so a cursor or an image anchored past the share clamps to
            // the share's end rather than dragging padding into the line.
            let mine = line_cells(&row_cells, self.continues(r));
            let mut glyphs: Vec<Cell> = Vec::new();
            for c in 0..self.cols {
                if !cursor_found && r == cur_row && c == cur_col {
                    cursor_found = true;
                    cursor_line = lines.len();
                    cursor_off = cur.len() + glyphs.len();
                }
                // An image anchored at this cell rides the rewrap by glyph offset (captured before
                // the trailer skip below, so an anchor on any cell — glyph or blank — is recorded).
                for &ai in &images_on_row[r as usize] {
                    if anchor_pos[ai].is_none() && self.images[ai].anchor.0 == c {
                        anchor_pos[ai] = Some((lines.len(), cur.len() + glyphs.len()));
                    }
                }
                let Some(cell) = mine.get(c as usize) else {
                    continue; // past this row's share of the line
                };
                if cell.width == Width::Trailer {
                    continue; // regenerated when the wide head is re-placed
                }
                glyphs.push(cell.clone());
            }
            let wrapped = self.wrapped(r);
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
        // Each entry: (cells, soft-wrap continuation, mark). The continuation carries the COLUMN
        // the re-break happened at, which is `buf.len()` and is NOT always the new width — a wide
        // cluster that will not fit breaks a column early. The mark rides only the FIRST physical
        // row of a logical line (its head) — where a prompt / output boundary sits.
        let mut phys: Vec<(Vec<Cell>, Option<u16>, Option<PromptMark>)> = Vec::new();
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
                    // The break puts exactly the cells already in `buf` on this row; `col` is that
                    // count (a wide cluster contributes its head AND its trailer to both).
                    let upto = u16::try_from(buf.len()).unwrap_or(cols);
                    phys.push((std::mem::take(&mut buf), Some(upto), None)); // soft-wrap break
                    col = 0;
                }
                buf.push(cell.clone());
                if cell.width == Width::Wide {
                    buf.push(Cell::trailer_for(cell));
                }
                // Record where an anchor at this glyph offset LANDS — after the wrap above, so the
                // (col, physical row) is the cell's real post-rewrap position, not its pre-wrap one.
                for (ai, pos) in anchor_pos.iter().enumerate() {
                    if anchor_phys[ai].is_none() && *pos == Some((li, i)) {
                        anchor_phys[ai] = Some((col, phys.len()));
                    }
                }
                col += w;
            }
            if cursor_phys.is_none() && cursor_line == li && cursor_off >= line.len() {
                cursor_phys = Some((col, phys.len()));
            }
            // An anchor offset past this line's content (a trimmed trailing blank) maps to its end.
            for (ai, pos) in anchor_pos.iter().enumerate() {
                if anchor_phys[ai].is_none()
                    && pos.is_some_and(|(l, off)| l == li && off >= line.len())
                {
                    anchor_phys[ai] = Some((col, phys.len()));
                }
            }
            phys.push((buf, None, None)); // hard end of this logical line
            // Re-attach the logical line's mark to its head physical row (always pushed above).
            phys[line_top].2 = line_marks[li];
        }
        // Pass 3 — materialize, bottom-anchored: the bottom `rows` physical rows
        // are visible; any overflow scrolls off the top into scrollback.
        let ncols = cols as usize;
        let keep = rows as usize;
        let total = phys.len();
        let start = total.saturating_sub(keep);
        // Inherited, and this is the site where losing it would bite hardest: the overflow below is
        // pushed through `push_scrollback` and then TRIMMED, so a re-defaulted limit would evict a
        // user's raised history on every reflow — silently, and only on resize.
        let mut next = Screen::new(cols, rows, self.history_limit);
        // The overflow above the visible window BECOMES the new scrollback. It already holds the
        // old scrollback (rewrapped as part of the unified stream), so do NOT also clone the old
        // deque — that would double the history. `Screen::new` leaves `next.scrollback` empty.
        for (cells, continues, mark) in phys.iter().take(start) {
            // Keep the styled cells (fg/bg/attrs) — scrollback paints in color — plus the row's
            // soft-wrap continuation (so a LATER reflow can rewrap it again) and its mark (so a
            // prompt rewrapped into overflow stays a jump target).
            next.push_scrollback(ScrollbackLine {
                cells: cells.clone(),
                continues: *continues,
                mark: *mark,
            });
        }
        next.trim_scrollback();
        debug_assert_eq!(
            next.scrollback_logical,
            next.scrollback
                .iter()
                .filter(|l| l.continues.is_none())
                .count(),
            "scrollback_logical stayed in sync with the deque across the rewrap"
        );
        // ⚠⚠ THE MONOTONIC TOTAL IS CARRIED, NOT RE-DERIVED — and it has to be set HERE, after the
        // loop above, precisely because that loop goes through `push_scrollback` and has therefore
        // just counted the RETAINED lines as if they were newly shed. A resize re-wraps ROWS; it
        // cannot create or destroy a LOGICAL line. Left re-derived, every reader's cursor would
        // jump backwards on each resize and the whole retained history would be delivered again —
        // which is the exact defect class this counter exists to end.
        next.logical_shed = self.logical_shed;
        for (out_r, (cells, continues, mark)) in phys[start..].iter().enumerate() {
            for (c, cell) in cells.iter().take(ncols).enumerate() {
                next.cells[out_r * ncols + c] = cell.clone();
            }
            next.continues[out_r] = *continues;
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
            blink: self.cursor.blink,
        };
        next.kind = self.kind;
        // Inline images ride the rewrap by ANCHOR CELL: each image's anchor is re-mapped to where
        // its cell lands after the re-break (the exact cell, tracked through the three passes like
        // the cursor — an image is a 2D placement, so it tracks its cell rather than re-attaching to
        // the line head the way a mark does). An image whose anchor cell reflows ABOVE the visible
        // window is evicted — the same no-scrollback-image bound as a scrolled-off image (Stage 3);
        // an anchor cell no longer in the reconstructed content (its row shrank away, or its
        // trailing-blank line was dropped) is likewise dropped rather than left at a stale margin.
        // `next_image_seq` carries so a re-transmit after the resize keeps a seq ABOVE every
        // surviving image's — a reset to 0 would read as STALE to a consumer tracking seq growth.
        next.next_image_seq = self.next_image_seq;
        // The content epoch likewise advances past this screen's — see the note in [`Self::resized`].
        // The rewrap already stamped every visible row through `stamp_row`, but the SCROLLBACK it
        // rebuilt was assigned wholesale rather than pushed, so the visible stamps alone would not
        // account for a history that changed width.
        next.content_epoch = next.content_epoch.max(self.content_epoch.wrapping_add(1));
        for (ai, img) in self.images.iter().enumerate() {
            if let Some((col, prow)) = anchor_phys[ai]
                && prow >= start
            {
                let mut moved = img.clone();
                moved.anchor = (
                    col.min(cols.saturating_sub(1)),
                    ((prow - start) as u16).min(rows.saturating_sub(1)),
                );
                next.images.push(moved);
            }
        }
        next
    }

    /// Drop all retained scrollback (the child sent `ESC [ 3 J`, ED-3). The SSOT for clearing
    /// scrollback, so the logical-line count resets with it.
    pub(crate) fn clear_scrollback(&mut self) {
        self.scrollback.clear();
        self.scrollback_logical = 0;
        // ⚠ `logical_shed` is deliberately NOT reset. The child cleared its HISTORY; it did not
        // un-print those lines. Resetting would rewind every reader's cursor into a past that no
        // longer exists and re-deliver whatever came next — see [`Self::lines_since`], where the
        // honest answer to "your cursor is behind what I kept" is a COUNT OF WHAT YOU LOST.
        self.touch_scrollback();
    }

    /// The number of COMPLETE logical lines currently in scrollback — the width-independent unit
    /// the [`history_limit`](Self::history_limit) bounds, unlike the physical row count [`Self::scrollback_len`]. A test
    /// introspection accessor (no non-test consumer yet); the count's SSOT is the field it reads.
    #[cfg(test)]
    #[must_use]
    pub(crate) fn scrollback_logical_len(&self) -> usize {
        self.scrollback_logical
    }

    /// This screen's [`content_epoch`](Self::content_epoch) — the O(1) "has anything
    /// [`history_bytes`](Self::history_bytes) would encode changed?" read.
    ///
    /// Compare two observations of it: EQUAL means nothing was mutated in between, so a re-encode is
    /// provably wasted. DIFFERENT means something was, though not necessarily that the encoded bytes
    /// differ (the counter is conservative — see the field). Never compare across screens: each screen
    /// counts its own mutations, so the value is only meaningful against an earlier read of the SAME
    /// screen.
    #[must_use]
    pub fn content_epoch(&self) -> u64 {
        self.content_epoch
    }

    /// Stamp row `row` dirty at `generation` and count the mutation — the ONE place a visible row's
    /// damage is recorded, so a writer cannot bump the row generation and forget the epoch (or the
    /// reverse). Out-of-range rows are ignored, exactly as the callers' own bounds checks did.
    fn stamp_row(&mut self, row: u16, generation: u64) {
        if let Some(slot) = self.generations.get_mut(row as usize) {
            *slot = generation;
            self.content_epoch = self.content_epoch.wrapping_add(1);
        }
    }

    /// Count a mutation of content the visible grid's row generations say nothing about: the
    /// SCROLLBACK (its three SSOT mutators) and the inline IMAGES (add / clear / delete). Both are
    /// encoded by [`Self::history_bytes`] and neither touches a row's damage stamp.
    fn touch_scrollback(&mut self) {
        self.content_epoch = self.content_epoch.wrapping_add(1);
    }

    /// Append one physical row to scrollback, maintaining the logical-line count. The SSOT for
    /// scrollback GROWTH — every push routes here so the count cannot desync; a row that ENDS a
    /// logical line (not soft-wrapped) adds one logical line.
    fn push_scrollback(&mut self, line: ScrollbackLine) {
        if line.continues.is_none() {
            self.scrollback_logical += 1;
            // ⚠ The MONOTONIC sibling: never decremented by a trim, never reset by a clear, and
            // carried verbatim across a reflow. It is what makes a reader's cursor an ADDRESS
            // rather than an offset into a window that moves under it. See [`Self::lines_since`].
            self.logical_shed += 1;
        }
        self.scrollback.push_back(line);
        self.touch_scrollback();
    }

    /// Evict the oldest scrollback until it fits BOTH bounds: this screen's
    /// [`history_limit`](Self::history_limit) LOGICAL lines (width-independent retention) AND the
    /// [`scrollback_physical_ceiling`] it derives (a memory guard against a pathological unbroken
    /// line). The SSOT for scrollback SHRINK — pops from the front (oldest), decrementing the
    /// logical count as each line-ending row leaves.
    ///
    /// Both bounds read the PER-SCREEN limit, so a raised `history-limit` raises them together; a
    /// version that kept the physical ceiling at the default's would cap the logical one at eight
    /// rows per line and make the setting look broken on wide, wrapped output.
    fn trim_scrollback(&mut self) {
        let physical_ceiling = scrollback_physical_ceiling(self.history_limit);
        while self.scrollback_logical > self.history_limit
            || self.scrollback.len() > physical_ceiling
        {
            match self.scrollback.pop_front() {
                Some(line) => {
                    if line.continues.is_none() {
                        self.scrollback_logical -= 1;
                    }
                    self.touch_scrollback();
                }
                None => break,
            }
        }
    }

    /// Scroll the [`ScrollRegion`] UP by `n`: the `n` rows leaving the top of the region are
    /// discarded (or retained as scrollback, see below) and the `n` rows vacated at the bottom
    /// become blank. Cells outside the region — above `top`, below `bottom`, and (under DECLRMM)
    /// left of `left` or right of `right` — are untouched. This is the scroll-region primitive
    /// behind IND / a line feed at the bottom margin, SU (`CSI S`), and DL (`CSI M`) — see
    /// [`crate::emulator`]. With the default full-screen region and `n == 1` it is the ordinary
    /// "output flows off the top" scroll.
    ///
    /// The rows leaving the top are pushed to the bounded scrollback FIFO — as STYLED
    /// cells, so history paints in its original colors — only when `to_scrollback` is set,
    /// the region is anchored at the screen top (`top == 0`), the region is FULL-WIDTH, and
    /// this is the MAIN screen. That is history genuinely leaving the top of the screen.
    /// `to_scrollback` is `true` for output-flow scrolls (a line feed at the bottom margin, SU)
    /// and `false` for the DL edit, which REMOVES lines rather than scrolling output away — so a
    /// DL at row 0 does not pollute the scrollback. A mid-screen region (`top > 0`) never reaches
    /// the scrollback regardless (those lines are interior, not off the top), and neither does a
    /// margined one: a row fragment is not a history line. Every row the op moves or blanks is
    /// damaged at `generation`.
    ///
    /// **Full-width vs banded.** A full-width region rotates whole rows — a memmove, allocating
    /// nothing — and carries the per-row metadata (soft-wrap flags, prompt marks) with them;
    /// blanked rows drop theirs. A BANDED region (DECLRMM in force) moves only the columns
    /// `[left, right]`, by element swaps, and leaves the per-row metadata where it is: a wrap
    /// flag and a prompt mark are facts about a WHOLE row, and half a row moving does not
    /// relocate them. The touched rows do drop their soft-wrap flag, because a row whose middle
    /// was scrolled out from under it no longer continues into the next one. A logical line
    /// soft-wrapped ACROSS a region boundary is a documented bound: scroll regions and reflow do
    /// not compose cleanly, and region-using apps position explicitly rather than relying on
    /// autowrap.
    pub(crate) fn scroll_region_up(
        &mut self,
        region: ScrollRegion,
        n: u16,
        to_scrollback: bool,
        generation: u64,
    ) {
        if self.rows == 0 || self.cols == 0 {
            return;
        }
        let top = region.top;
        let bottom = region.bottom.min(self.rows - 1);
        let left = region.left;
        let right = region.right.min(self.cols - 1);
        if top > bottom || left > right {
            return;
        }
        let height = bottom - top + 1;
        let n = n.min(height); // scrolling by >= the region height blanks it whole
        if n == 0 {
            return;
        }
        let full_width = region.full_width(self.cols);
        // An image tracks the grid like a cell: its anchor scrolls up with the region. Do this
        // FIRST, before the row-clear below blanks the vacated rows (post-shift no image sits there,
        // so `clear_row`'s own image-drop is a no-op here). See [`Self::shift_images_up`].
        self.shift_images_up(top, bottom, left, right, n);
        if !full_width {
            self.scroll_band_up(top, bottom, left, right, n, generation);
            return;
        }
        // Retain the rows leaving the top (`[top, top+n)`) as history, oldest first, only
        // for an output-flow scroll of a top-anchored FULL-WIDTH region on the main screen.
        if to_scrollback && top == 0 && self.kind == ScreenKind::Main {
            for r in 0..n {
                // Carry the row's shell-integration mark AND its soft-wrap flag into history WITH
                // its cells, so a prompt that scrolls off the top stays a jump target and a
                // soft-wrapped line stays one logical line for a later reflow ([`ScrollbackLine`]).
                // Build the line first (immutable borrows) before the `&mut self` push.
                let line = ScrollbackLine {
                    cells: self.row_cells(r),
                    continues: self.continues[r as usize],
                    mark: self.marks[r as usize],
                };
                self.push_scrollback(line);
            }
            self.trim_scrollback();
        }
        let cols = self.cols as usize;
        let shift = height - n; // rows that survive and move up by `n`
        // Move the surviving rows up by `n` as an in-place slice ROTATION, not a per-cell clone:
        // each [`Cell`] owns a heap `cluster`, so cloning every cell of every scrolled line made a
        // bulk scroll O(cells) heap allocations — a throughput wall on `cat`-style output (a screen
        // of continuous text scrolled ~74 KiB/s). A rotation is memmove-cheap and allocates nothing.
        // The already-evicted top `n` rows land at the bottom, to be blanked next.
        let span = (top as usize * cols)..((bottom as usize + 1) * cols);
        self.cells[span].rotate_left(n as usize * cols);
        // The per-row metadata rotates in lockstep (cheap Copy scalars).
        self.continues[top as usize..=bottom as usize].rotate_left(n as usize);
        self.marks[top as usize..=bottom as usize].rotate_left(n as usize);
        // The surviving rows are dirty at the new generation.
        for r in top..top + shift {
            self.stamp_row(r, generation);
        }
        // Blank the `n` rows vacated at the bottom of the region (this also resets their
        // wrapped/mark/generation, overwriting whatever the rotation parked there).
        for i in 0..n {
            self.clear_row(top + shift + i, generation);
        }
    }

    /// Scroll the [`ScrollRegion`] DOWN by `n`: the `n` rows leaving the bottom of the region are
    /// discarded and the `n` rows vacated at the top become blank. Cells outside the region are
    /// untouched. The mirror of [`Self::scroll_region_up`] behind RI / a reverse index at the top
    /// margin, SD (`CSI T`), and IL (`CSI L`). A down scroll never reaches the scrollback — it
    /// discards the bottom, not the top. Full-width and banded behave exactly as documented on
    /// [`Self::scroll_region_up`]. Every moved or blanked row is damaged at `generation`.
    pub(crate) fn scroll_region_down(&mut self, region: ScrollRegion, n: u16, generation: u64) {
        if self.rows == 0 || self.cols == 0 {
            return;
        }
        let top = region.top;
        let bottom = region.bottom.min(self.rows - 1);
        let left = region.left;
        let right = region.right.min(self.cols - 1);
        if top > bottom || left > right {
            return;
        }
        let height = bottom - top + 1;
        let n = n.min(height);
        if n == 0 {
            return;
        }
        // An image's anchor scrolls down with the region (mirror of [`Self::scroll_region_up`]).
        self.shift_images_down(top, bottom, left, right, n);
        if !region.full_width(self.cols) {
            self.scroll_band_down(top, bottom, left, right, n, generation);
            return;
        }
        let cols = self.cols as usize;
        // Move the surviving rows down by `n` as an in-place slice ROTATION (mirror of
        // [`Self::scroll_region_up`] — no per-cell clone, so a reverse-scroll allocates nothing).
        // The bottom `n` rows wrap to the top, to be blanked next.
        let span = (top as usize * cols)..((bottom as usize + 1) * cols);
        self.cells[span].rotate_right(n as usize * cols);
        self.continues[top as usize..=bottom as usize].rotate_right(n as usize);
        self.marks[top as usize..=bottom as usize].rotate_right(n as usize);
        // The surviving rows (now at `[top + n, bottom]`) are dirty at the new generation.
        for r in (top + n)..=bottom {
            self.stamp_row(r, generation);
        }
        // Blank the `n` rows vacated at the top of the region.
        for i in 0..n {
            self.clear_row(top + i, generation);
        }
    }

    /// The BANDED half of [`Self::scroll_region_up`]: move only the columns `[left, right]` of the
    /// rows `[top, bottom]` up by `n`, blanking the band in the `n` rows vacated at the bottom.
    ///
    /// A whole-row rotation is unavailable here — the untouched columns either side must stay put —
    /// so the move is a walk of element SWAPS, which is still allocation-free: a [`Cell`] owns a
    /// heap cluster, and swapping moves the ownership rather than cloning it. Walking `r` upward
    /// while swapping `band(r)` with `band(r + n)` leaves the surviving bands in order at the top
    /// and parks the evicted ones at the bottom, exactly as `rotate_left` would.
    ///
    /// The per-row metadata does NOT move (see [`Self::scroll_region_up`]), but every touched row
    /// drops its soft-wrap flag: a row whose middle was scrolled out from under it no longer
    /// continues into the next one, and a stale flag would make a later reflow join two unrelated
    /// lines.
    fn scroll_band_up(
        &mut self,
        top: u16,
        bottom: u16,
        left: u16,
        right: u16,
        n: u16,
        generation: u64,
    ) {
        let cols = self.cols as usize;
        let shift = (bottom - top + 1) - n;
        for r in top..top + shift {
            let (src, dst) = ((r + n) as usize * cols, r as usize * cols);
            for c in left as usize..=right as usize {
                self.cells.swap(dst + c, src + c);
            }
        }
        // Blank the band in the `n` rows vacated at the bottom.
        for r in top + shift..=bottom {
            let base = r as usize * cols;
            for c in left as usize..=right as usize {
                self.cells[base + c] = Cell::blank();
            }
        }
        for r in top..=bottom {
            self.continues[r as usize] = None;
            self.stamp_row(r, generation);
        }
    }

    /// The BANDED half of [`Self::scroll_region_down`] — the mirror of [`Self::scroll_band_up`],
    /// walking `r` DOWNWARD so a source band is read before it is overwritten.
    fn scroll_band_down(
        &mut self,
        top: u16,
        bottom: u16,
        left: u16,
        right: u16,
        n: u16,
        generation: u64,
    ) {
        let cols = self.cols as usize;
        for r in ((top + n)..=bottom).rev() {
            let (src, dst) = ((r - n) as usize * cols, r as usize * cols);
            for c in left as usize..=right as usize {
                self.cells.swap(dst + c, src + c);
            }
        }
        // Blank the band in the `n` rows vacated at the top.
        for r in top..top + n {
            let base = r as usize * cols;
            for c in left as usize..=right as usize {
                self.cells[base + c] = Cell::blank();
            }
        }
        for r in top..=bottom {
            self.continues[r as usize] = None;
            self.stamp_row(r, generation);
        }
    }

    /// Shift every inline image ANCHORED INSIDE the scrolled region UP by `n` — the image tracks
    /// its text (a sixel scrolls with the output, R1404 Stage 3). An image anchored in the `n` rows
    /// leaving the top of the region (`[top, top+n)`) is EVICTED, exactly as those rows' cells
    /// leave; one anchored outside the region — including outside its COLUMNS when DECLRMM has
    /// narrowed it — is untouched, because those cells did not move. Anchor-granular (an image
    /// straddling a region boundary tracks by its anchor cell — a documented bound).
    /// Scrollback-image retention (re-appearing when you scroll back up) is a deferred bound: a
    /// scrolled-off-the-top image is dropped, not kept.
    fn shift_images_up(&mut self, top: u16, bottom: u16, left: u16, right: u16, n: u16) {
        self.images.retain_mut(|img| {
            let (c, r) = img.anchor;
            if r < top || r > bottom || c < left || c > right {
                true // outside the scrolled region — unmoved
            } else if r < top + n {
                false // in the rows leaving the top — evicted
            } else {
                img.anchor.1 = r - n;
                true
            }
        });
    }

    /// Shift every inline image anchored inside the region DOWN by `n`, evicting one that leaves
    /// the bottom — the mirror of [`Self::shift_images_up`] (RI / SD / IL).
    fn shift_images_down(&mut self, top: u16, bottom: u16, left: u16, right: u16, n: u16) {
        self.images.retain_mut(|img| {
            let (c, r) = img.anchor;
            if r < top || r > bottom || c < left || c > right {
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

    /// Set the logical-pixel extent of one cell (`width`, `height`) — the DISPLAY geometry the
    /// host sources from the GUI's font metrics (the emulator runs in the daemon and has none of
    /// its own). It feeds the XTWINOPS PIXEL reports (`14 t` / `15 t` / `16 t`) and, via
    /// [`Self::cell_pixel_size`], the PTY winsize `xpixel` / `ypixel` the host derives so a child
    /// sizes images correctly. `0` on either axis means "unknown" (the reports fall back to the `0`
    /// sentinel). It is display geometry, not the child's — RIS preserves it, like `cols` / `rows`.
    fn set_cell_pixel_size(&mut self, width: u16, height: u16);

    /// The logical-pixel extent of one cell (`width`, `height`) last set via
    /// [`Self::set_cell_pixel_size`], or `(0, 0)` while unknown. The host reads it to derive the
    /// PTY winsize `xpixel` / `ypixel` (`cols * width`, `rows * height`).
    fn cell_pixel_size(&self) -> (u16, u16);

    /// Note that the CONSUMER has just sent the child input (a keystroke, a paste, an
    /// injected key). This ends the resize-redraw reinterpretation epoch (see the emulator's
    /// `in_resize_redraw`): the user acting — typing at, or submitting from, the prompt — is the
    /// definitive end of the line editor's `SIGWINCH` redraw, so a submitting `CR LF` and the
    /// command output after it are hard line breaks again rather than soft wraps. The transport
    /// calls this on the input path BEFORE writing the bytes to the child, so the child's response
    /// is always emulated with the epoch already closed (no read-vs-write race). It is a no-op
    /// when no redraw epoch is open. Automated child replies (device / clipboard answers) do NOT
    /// call it — they are not the user acting.
    fn note_input(&mut self);

    /// The current authoritative screen.
    fn screen(&self) -> &Screen;

    /// The terminal's live colour [`Palette`] — the SSOT the OSC colour commands
    /// mutate, read by the projection (sprag-grid's `project`) to resolve each cell's
    /// [`Color`] to a concrete RGB. Terminal-wide (shared by the main and alt
    /// screens), so it lives beside the emulator, not on a [`Screen`].
    fn palette(&self) -> &Palette;

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

    /// Whether SYNCHRONIZED OUTPUT (DEC private mode 2026) is currently active — the child has
    /// opened an atomic-frame update (`CSI ? 2026 h`) and not yet closed it (`CSI ? 2026 l`). While
    /// this is `true`, a display client MUST NOT present intermediate state: it holds its repaint
    /// so the whole batch of screen changes lands as ONE frame (the tearing-free redraw neovim /
    /// notcurses / fzf rely on). The [`Screen`] is mutated as usual regardless — this only gates
    /// PRESENTATION — so a consumer honors it by deferring its repaint wake while set and repainting
    /// once when it clears (the reader loop's `on_dirty` gate). A never-closed update is the child's
    /// bug; a robust client MAY add a safety deadline (a held frame is flushed after a short timeout)
    /// — that timing policy is the display's, not the emulator's (which owns no clock).
    fn synchronized_output(&self) -> bool;
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

    /// A match that lies within ONE row of a line that occupies one row — the ordinary case, where
    /// the logical line, the row it is on and the whole match are the same span.
    ///
    /// Deliberately NOT usable for a wrapping match: those are spelled out field by field, because
    /// the row a match starts on and the widths it covers are exactly what a helper would hide.
    fn hit(line: usize, col: u16, cols: u16) -> FindMatch {
        FindMatch {
            line,
            row: line,
            col,
            cols,
            wrapped: Vec::new(),
        }
    }

    /// [`em`] on an emulator configured for `limit` logical lines of history — the constructor a
    /// daemon uses once it has read the user's `history-limit`.
    fn em_limited(cols: u16, rows: u16, limit: usize, bytes: &str) -> Emulator {
        let mut e = Emulator::with_history_limit(cols, rows, limit);
        e.advance(bytes.as_bytes());
        e
    }

    /// `n` numbered lines, each ending in a hard newline — `n` LOGICAL lines on a screen wide
    /// enough not to wrap them.
    fn numbered_lines(n: usize) -> String {
        (0..n).map(|i| format!("{i}\r\n")).collect()
    }

    /// The palette resolves the three colour forms and the standard-xterm indexed ranges — the
    /// formulas must match pinion's `Palette` byte-for-byte, so an un-mutated palette projects
    /// identically to the pre-OSC-colour behaviour.
    #[test]
    fn palette_resolves_xterm_colors() {
        let p = Palette::xterm_default();
        // Rgb passes through; Default consults the per-target dynamic colour.
        assert_eq!(
            p.resolve(Color::Rgb(Rgb::new(1, 2, 3)), ColorTarget::Foreground),
            Rgb::new(1, 2, 3)
        );
        assert_eq!(
            p.resolve(Color::Default, ColorTarget::Foreground),
            Rgb::new(0xe5, 0xe5, 0xe5),
            "default fg = xterm ANSI 7"
        );
        assert_eq!(
            p.resolve(Color::Default, ColorTarget::Background),
            Rgb::new(0x00, 0x00, 0x00),
            "default bg = xterm ANSI 0"
        );
        // ANSI base (index 1 = red), the 6x6x6 cube, and the grayscale ramp.
        assert_eq!(p.indexed(1), Rgb::new(0xcd, 0x00, 0x00));
        assert_eq!(
            p.indexed(196),
            Rgb::new(0xff, 0x00, 0x00),
            "cube 16+180 = pure red"
        );
        assert_eq!(
            p.indexed(232),
            Rgb::new(0x08, 0x08, 0x08),
            "grayscale ramp start"
        );
        assert_eq!(
            p.indexed(255),
            Rgb::new(0xee, 0xee, 0xee),
            "grayscale ramp end"
        );
    }

    /// `OSC 4` / `OSC 104` override and reset a single index without disturbing the others; the
    /// whole-palette reset restores the xterm seed.
    #[test]
    fn palette_index_override_and_reset() {
        let mut p = Palette::xterm_default();
        p.set_indexed(1, Rgb::new(0x11, 0x22, 0x33));
        assert_eq!(p.indexed(1), Rgb::new(0x11, 0x22, 0x33));
        assert_eq!(
            p.indexed(2),
            Rgb::new(0x00, 0xcd, 0x00),
            "a sibling index is untouched"
        );
        p.reset_indexed(1);
        assert_eq!(
            p.indexed(1),
            Rgb::new(0xcd, 0x00, 0x00),
            "reset restores xterm red"
        );
        // Whole-palette reset after two overrides.
        p.set_indexed(5, Rgb::new(1, 1, 1));
        p.set_indexed(200, Rgb::new(2, 2, 2));
        p.reset_all_indexed();
        assert_eq!(p.indexed(5), Palette::xterm_indexed(5));
        assert_eq!(p.indexed(200), Palette::xterm_indexed(200));
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
        let n = DEFAULT_SCROLLBACK_LINES + 100;
        let input: String = (0..n).map(|i| format!("{i}\r\n")).collect();
        let e = em(12, 1, &input);
        assert_eq!(
            e.screen().scrollback_len(),
            DEFAULT_SCROLLBACK_LINES,
            "bounded"
        );
        // The oldest 100 lines (0..100) were dropped; 100 is now the oldest.
        assert_eq!(e.screen().scrollback_rows().next().as_deref(), Some("100"));
    }

    #[test]
    fn scrollback_cap_counts_logical_lines_not_physical_rows() {
        // The cap is LOGICAL lines (tmux's width-independent model), not physical rows. Fill it,
        // then narrow so every line wraps to two physical rows: the physical row count doubles PAST
        // the old physical cap, yet NO logical line is evicted — a physical-row cap would have
        // halved the retained history the moment the rows doubled.
        let n = DEFAULT_SCROLLBACK_LINES + 5;
        let input: String = (0..n).map(|i| format!("L{i:07}\r\n")).collect(); // 8 glyphs/line
        let mut e = em(16, 2, &input); // width 16: each logical line is one physical row
        assert_eq!(
            e.screen().scrollback_logical_len(),
            DEFAULT_SCROLLBACK_LINES
        );
        e.resize(4, 2); // narrow: each 8-glyph line wraps to two physical rows
        assert_eq!(
            e.screen().scrollback_logical_len(),
            DEFAULT_SCROLLBACK_LINES,
            "narrowing evicted no logical line (width-independent retention)"
        );
        assert!(
            e.screen().scrollback_len() > DEFAULT_SCROLLBACK_LINES,
            "physical rows doubled past the old physical cap, yet history was retained"
        );
        e.resize(16, 2); // widen back
        assert_eq!(
            e.screen().scrollback_logical_len(),
            DEFAULT_SCROLLBACK_LINES,
            "the logical count is stable across narrow∘widen"
        );
    }

    #[test]
    fn a_pathological_unbroken_line_is_bounded_by_the_physical_ceiling() {
        // One logical line of many thousands of glyphs (no newline) autowraps to one physical row
        // per glyph on a width-1 screen — a SINGLE logical line, so the LOGICAL cap never bounds
        // it. Only the physical ceiling does, so a runaway line cannot pin unbounded memory.
        let huge = "a".repeat(scrollback_physical_ceiling(DEFAULT_SCROLLBACK_LINES) + 100);
        let e = em(1, 2, &huge);
        assert!(
            e.screen().scrollback_len() <= scrollback_physical_ceiling(DEFAULT_SCROLLBACK_LINES),
            "the physical ceiling bounds a pathological unbroken line"
        );
    }

    #[test]
    fn a_configured_limit_governs_retention_in_both_directions() {
        // The point of the option: a limit BELOW the default must actually evict, and one ABOVE it
        // must actually retain. Asserting only the raised direction would pass on a screen that
        // ignored the limit and kept everything.
        // Every line is newline-TERMINATED, so all 100 scroll off a 1-row screen and the visible
        // row ends blank; the newest 10 of 0..=99 are therefore 90..=99.
        let small = em_limited(12, 1, 10, &numbered_lines(100));
        assert_eq!(small.screen().scrollback_logical_len(), 10, "10 retained");
        assert_eq!(
            small.screen().scrollback_rows().next().as_deref(),
            Some("90"),
            "the OLDEST survivor is line 90 — eviction takes from the front",
        );

        // Above the default, where a screen still enforcing `DEFAULT_SCROLLBACK_LINES` would stop.
        let big = em_limited(12, 1, 2_500, &numbered_lines(2_000));
        assert_eq!(
            big.screen().scrollback_logical_len(),
            2_000,
            "everything fed is retained past the 1000-line default",
        );
    }

    #[test]
    fn a_zero_limit_retains_no_history_at_all() {
        // `0` is a VALUE, not an unset: the pane remembers nothing. A screen that treated it as
        // "unset, use the default" would retain 1000 lines here, which is the opposite of the ask.
        // The trailing text has no newline, so it stays on the VISIBLE row: a zero limit throws
        // history away, it does not stop the terminal from being a terminal.
        let e = em_limited(12, 1, 0, &format!("{}live", numbered_lines(50)));
        assert_eq!(e.screen().scrollback_logical_len(), 0);
        assert_eq!(e.screen().scrollback_len(), 0, "no physical rows either");
        assert_eq!(
            e.screen().row_text(0),
            "live",
            "the VISIBLE row is untouched"
        );
    }

    #[test]
    fn the_physical_ceiling_scales_with_the_configured_limit() {
        // The ceiling is DERIVED per screen. Held at the default's 8x1000, a pane configured for
        // 50 lines of pathological unbroken output would be bounded at 8000 physical rows instead
        // of its own 400 — the guard would stop tracking the setting it exists beside.
        let huge = "a".repeat(1_000);
        let e = em_limited(1, 2, 50, &huge);
        assert!(
            e.screen().scrollback_len() <= scrollback_physical_ceiling(50),
            "bounded by 8x50, not by 8x the default: {} rows",
            e.screen().scrollback_len(),
        );
    }

    #[test]
    fn the_verbatim_resize_fallback_carries_the_limit_too() {
        // `resized` is the fallback `reflowed` takes for the alt screen and for a degenerate size.
        // Driven through `Emulator::resize` it is hard to observe — the alt screen holds no
        // scrollback for a wrong limit to evict — so it is called DIRECTLY here rather than left as
        // an inheritance nothing checks. It carries scrollback across verbatim WITHOUT trimming, so
        // a re-defaulted limit would sit dormant in the new screen and evict on the next push
        // instead of at the resize, which is the kind of delay that makes a bug hard to attribute.
        let mut screen = Screen::new(16, 2, 2_500);
        for i in 0..1_500 {
            screen.push_scrollback(ScrollbackLine {
                cells: vec![Cell::blank()],
                continues: None,
                mark: None,
            });
            let _ = i;
        }
        screen.trim_scrollback();
        assert_eq!(
            screen.scrollback_logical_len(),
            1_500,
            "nothing evicted yet"
        );

        let mut next = screen.resized(8, 2);
        assert_eq!(next.history_limit(), 2_500, "the limit came across");
        next.trim_scrollback();
        assert_eq!(
            next.scrollback_logical_len(),
            1_500,
            "and it still governs: a re-defaulted 1000 would have evicted 500 lines here",
        );
    }

    #[test]
    fn a_saved_history_holds_the_visible_screen_whatever_the_retention_limit() {
        // A saved history is the scrollback PLUS the visible screen, so the retention limit is the
        // wrong bound for it. At `history-limit 0` there is no scrollback and the pane must still
        // come back showing what was on it; a save budget narrowed to the retention limit would
        // encode nothing here, blanking the restored pane.
        // One row, so the first line genuinely scrolls off rather than staying on screen.
        let e = em_limited(20, 1, 0, "gone\r\nstill here");
        let bytes = e
            .screen()
            .history_bytes(HistoryLimits::text_only(usize::MAX));
        let replayed = String::from_utf8_lossy(&bytes).into_owned();
        assert!(
            replayed.contains("still here"),
            "the visible screen must survive a zero retention limit: {replayed:?}",
        );
        assert!(
            !replayed.contains("gone"),
            "and the line that scrolled off must NOT, since nothing retained it: {replayed:?}",
        );
    }

    #[test]
    fn an_unbounded_save_budget_persists_everything_a_raised_limit_kept() {
        // The reboot half of the option. With no operator ceiling the encoder saturates at what the
        // screen holds, so a pane configured deeper than the old 1000-line default saves its full
        // depth — a budget still fixed at that default would silently drop the difference, and only
        // across a restart.
        let e = em_limited(16, 2, 2_500, &numbered_lines(2_000));
        let bytes = e
            .screen()
            .history_bytes(HistoryLimits::text_only(usize::MAX));

        // Replayed into a fresh emulator, which is what a restore actually does — asserting on the
        // encoded BYTES would be asserting on escape sequences rather than on what comes back.
        let mut restored = Emulator::with_history_limit(16, 2, 5_000);
        restored.advance(&bytes);
        assert_eq!(
            restored.screen().scrollback_rows().next().as_deref(),
            Some("0"),
            "the OLDEST line survived, so nothing was truncated to the 1000-line default",
        );
        assert_eq!(
            restored.screen().scrollback_logical_len(),
            e.screen().scrollback_logical_len(),
            "and the depth came back exactly",
        );
    }

    #[test]
    fn a_derived_screen_inherits_the_configured_limit() {
        // Every screen a resize or a reflow produces is a fresh `Screen::new`, so each is a place
        // the limit could be re-defaulted — which would silently evict a raised history on the
        // next resize, and only then. Both paths are exercised: `reflowed` (main screen, rewraps)
        // and `resized` (the alt-screen/degenerate fallback, verbatim).
        let mut e = em_limited(16, 2, 2_500, &numbered_lines(2_000));
        assert_eq!(e.screen().history_limit(), 2_500);
        let before = e.screen().scrollback_logical_len();

        e.resize(8, 2); // narrow — the main-screen REFLOW path
        assert_eq!(e.screen().history_limit(), 2_500, "carried across a reflow");
        assert_eq!(
            e.screen().scrollback_logical_len(),
            before,
            "a re-defaulted limit would have evicted down to 1000 here",
        );

        // The alt screen is its own `Screen::new`, and main is restored from it.
        e.advance(b"\x1b[?1049h");
        assert_eq!(e.screen().history_limit(), 2_500, "the alt screen too");
        e.resize(20, 2); // resize WHILE in alt — the verbatim `resized` path
        e.advance(b"\x1b[?1049l");
        assert_eq!(
            e.screen().scrollback_logical_len(),
            before,
            "main came back through the alt round-trip with its history intact",
        );
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
    fn resize_grow_reclaims_scrollback_into_the_visible_area() {
        // (8,2): "a" scrolled off, "b"/"c" visible. Growing to 4 rows pulls "a" back down
        // into view (xterm/kitty reclaim; tmux does not) — scrollback empties, no history lost.
        let mut e = em(8, 2, "a\r\nb\r\nc");
        assert_eq!(e.screen().scrollback_rows().collect::<Vec<_>>(), ["a"]);
        e.resize(12, 4);
        assert_eq!(
            e.screen().scrollback_len(),
            0,
            "the grown height reclaimed the scrolled-off line"
        );
        assert_eq!(e.screen().row_text(0), "a", "history pulled back into view");
        assert_eq!(e.screen().row_text(1), "b");
        assert_eq!(e.screen().row_text(2), "c");
        assert_eq!(e.screen().full_text(), "a\nb\nc", "no history lost");
    }

    /// ⚠⚠ **A LINE NUMBER SURVIVES A RESIZE, WHICH IS THE WHOLE REASON THE UNIT IS A LOGICAL LINE.**
    ///
    /// A reader holding a cursor is holding an ADDRESS. If a resize could change what line 2 means,
    /// the address would be an offset into a window that moves, and every consumer would either
    /// re-deliver its source's history or lose its place — the defect class this exists to end.
    ///
    /// Reflow re-wraps ROWS and cannot create or destroy a LOGICAL line, so the fixture reads the
    /// same lines at three different widths from the same cursor, including one line that is
    /// physically ONE row at width 8 and TWO at width 4.
    #[test]
    fn a_line_keeps_its_number_and_its_text_across_a_resize() {
        let mut e = em(8, 2, "abcdef\r\n1\r\n2\r\n3");
        let wide = e.screen().lines_since(0);
        assert_eq!(
            wide.lines,
            ["abcdef", "1", "2"],
            "two shed lines and the one COMPLETE visible row — `3` is the cursor's own line and is \
             still being written",
        );
        assert_eq!(wide.lost, 0);

        e.resize(4, 2);
        let narrow = e.screen().lines_since(0);
        assert_eq!(
            narrow.lines, wide.lines,
            "⚠⚠ the SAME lines at half the width — `abcdef` is one row at 8 and two at 4, and a \
             reader must not be told the difference",
        );
        assert_eq!(
            narrow.next, wide.next,
            "and the cursor to resume from is the same number, or every resize would re-deliver \
             the history",
        );

        e.resize(20, 2);
        assert_eq!(
            e.screen().lines_since(0).lines,
            wide.lines,
            "and widening rejoins it without inventing a line either",
        );
        // Resuming from the mid-stream cursor yields only what follows it, at any width.
        assert_eq!(e.screen().lines_since(1).lines, ["1", "2"]);
        assert_eq!(e.screen().lines_since(3).lines, Vec::<String>::new());

        // ⚠⚠ AND THE HALF THAT MEASURES THE CARRY. Above, everything ever shed is still retained,
        // so a reflow that RE-DERIVED the total from the rows it re-pushes would arrive at the
        // same number and this gate would pass while covering nothing — measured: removing the
        // carry left the assertions above green. Once a TRIM has evicted lines the two answers
        // diverge, and re-deriving rewinds every reader's cursor by exactly the evicted count.
        let mut small = Emulator::with_history_limit(8, 2, 2);
        small.advance(b"1\r\n2\r\n3\r\n4\r\n5\r\n6");
        let before = small.screen().lines_since(u64::MAX).next;
        small.resize(4, 2);
        assert_eq!(
            small.screen().lines_since(u64::MAX).next,
            before,
            "a resize must not rewind the numbering of a screen whose history has been TRIMMED — \
             re-deriving the total from the retained rows loses every evicted line's address, and \
             a reader resuming from its cursor is handed the retained history all over again",
        );
    }

    /// ⚠⚠ **A LINE WITH NO NEWLINE AFTER IT IS OFFERED SEPARATELY, NEVER COUNTED.**
    ///
    /// A reply need not end in a newline, and for a one-shot tool that unfinished line is usually
    /// the whole ANSWER. Folding it into [`LinesSince::lines`] would hand consumers half a sentence
    /// as though the child had finished it; dropping it would silently lose the last thing the
    /// program said. So it is carried apart, and the cursor does NOT advance past it — a consumer
    /// that ignores it loses nothing and is handed the line whole once it is terminated.
    ///
    /// Three halves: it is offered, it is EXCLUDED from the complete lines and the cursor, and
    /// terminating it moves it across without duplicating it.
    #[test]
    fn an_unterminated_line_is_offered_apart_and_counted_only_once_it_ends() {
        let mut e = em(20, 3, "done\r\nhalf");
        let seen = e.screen().lines_since(0);
        assert_eq!(seen.lines, ["done"], "only what the child finished saying");
        assert_eq!(
            seen.partial, "half",
            "and the line it is still on, offered apart",
        );
        let mark = seen.next;

        // ⚠ THE HALF THAT STOPS A CONSUMER BEING FED IT TWICE. A reader that took the partial and
        // advanced past it would lose the rest of the line; one that took it and did NOT would see
        // it again — so the cursor must sit BEFORE it, and terminating must yield it once, whole.
        e.advance(b" of it\r\n");
        let after = e.screen().lines_since(mark);
        assert_eq!(
            after.lines,
            ["half of it"],
            "terminating the line hands it over ONCE and entire",
        );
        assert_eq!(after.partial, "", "with nothing left in progress");
        assert_eq!(
            e.screen().lines_since(after.next).lines,
            Vec::<String>::new(),
            "and a caller that is caught up is handed nothing",
        );
    }

    /// ⚠⚠ **A READER THAT WAS TOO SLOW IS TOLD HOW MUCH IT MISSED.**
    ///
    /// Scrollback is bounded, so a consumer that stays away longer than the history is deep cannot
    /// be given what it missed. **The alternative is a silent gap, which looks exactly like a quiet
    /// source** — and a relay cannot tell *nothing happened* from *I was not fast enough*.
    ///
    /// ⚠ The count is what a caller can act on: it is the number of complete lines that existed and
    /// were evicted, not a flag saying something went wrong.
    #[test]
    fn a_cursor_behind_the_retained_history_is_told_what_it_lost() {
        // A two-line history on a two-row screen: print well past both.
        let mut e = Emulator::with_history_limit(8, 2, 2);
        e.advance(b"1\r\n2\r\n3\r\n4\r\n5\r\n6");
        let all = e.screen().lines_since(0);
        assert!(all.lost > 0, "the fixture must actually overrun: {all:?}");
        assert_eq!(
            u64::try_from(all.lines.len()).unwrap() + all.lost,
            all.next,
            "⚠ EVERY line is accounted for: what was handed over plus what was lost IS the address \
             the reader resumes from — a reader can never silently skip one",
        );
        assert!(
            !all.lines.iter().any(|line| line == "1"),
            "the earliest lines are genuinely gone rather than quietly re-numbered: {all:?}",
        );
        assert_eq!(
            e.screen().lines_since(all.next).lines,
            Vec::<String>::new(),
            "and a caller that is caught up is handed nothing",
        );
    }

    /// ⚠⚠ **CLEARING THE HISTORY DOES NOT UN-PRINT WHAT WAS PRINTED.**
    ///
    /// `ESC [ 3 J` drops retained scrollback. Resetting the line numbering with it would rewind
    /// every reader's cursor into a past that no longer exists, and everything printed afterwards
    /// would be delivered a second time — so the monotonic count survives the clear, and a reader
    /// whose cursor is now behind the retained history learns that as a LOSS.
    #[test]
    fn clearing_the_scrollback_does_not_rewind_the_line_numbering() {
        let mut e = em(8, 2, "1\r\n2\r\n3\r\n4");
        let before = e.screen().lines_since(0).next;
        assert!(before >= 3, "the fixture must have shed lines: {before}");
        e.advance(b"\x1b[3J");
        assert_eq!(
            e.screen().lines_since(0).next,
            before,
            "the child cleared its HISTORY, not its past — the numbering must not restart",
        );
        assert!(
            e.screen().lines_since(0).lost > 0,
            "and a reader still at line 0 is told the lines it wanted are gone",
        );
    }

    #[test]
    fn narrow_rewraps_a_scrolled_off_logical_line() {
        // "abcdef" is a 6-glyph logical line that scrolled off at width 8 (one physical row).
        // Narrowing to width 4 must REWRAP it in history — before this it stayed frozen at the
        // old margin. "abcdef" re-breaks to "abcd"/"ef" (a soft wrap), then "1" fills scrollback;
        // "2"/"3" are the visible rows.
        let mut e = em(8, 2, "abcdef\r\n1\r\n2\r\n3"); // scrollback ["abcdef","1"]
        assert_eq!(
            e.screen().scrollback_rows().collect::<Vec<_>>(),
            ["abcdef", "1"]
        );
        e.resize(4, 2);
        assert_eq!(
            e.screen().scrollback_rows().collect::<Vec<_>>(),
            ["abcd", "ef", "1"],
            "the scrolled-off logical line rewrapped to the narrow width"
        );
        assert_eq!(e.screen().row_text(0), "2");
        assert_eq!(e.screen().row_text(1), "3");
    }

    #[test]
    fn scrollback_rewrap_round_trips_via_the_preserved_wrap_flag() {
        // Narrow (rewraps "abcdef" into "abcd"+"ef" in scrollback, "abcd" flagged soft-wrapped),
        // then widen back: the logical line REJOINS only because the scrollback wrap flag survived.
        // Revert-proof for `ScrollbackLine::wrapped`: drop the flag and the rejoined text differs.
        let mut e = em(8, 2, "abcdef\r\n1\r\n2\r\n3");
        let original = e.screen().full_text();
        e.resize(4, 2); // narrow: "abcdef" -> "abcd"(soft-wrap) + "ef" in history
        e.resize(8, 2); // widen: the soft-wrapped history line must rejoin to "abcdef"
        assert_eq!(
            e.screen().full_text(),
            original,
            "widen∘narrow restores the history text (scrollback wrap preserved)"
        );
        assert_eq!(
            e.screen().scrollback_rows().collect::<Vec<_>>(),
            ["abcdef", "1"],
            "the rewrapped history rejoined into one line"
        );
    }

    #[test]
    fn rewrapped_scrollback_keeps_its_prompt_mark_on_the_line_head() {
        // A prompt (OSC 133 ;A) on a 6-glyph line scrolls off, then a narrow rewraps it. The mark
        // must re-attach to the rewrapped line's HEAD ("abcd"), not its continuation ("ef") — so a
        // scrolled-off prompt stays a jump target across a resize.
        let mut e = em(8, 2, "\x1b]133;A\x1b\\abcdef\r\n1\r\n2\r\n3");
        assert_eq!(e.screen().scrollback_mark(0), Some(PromptMark::Prompt));
        e.resize(4, 2);
        assert_eq!(
            e.screen().scrollback_mark(0),
            Some(PromptMark::Prompt),
            "the mark rode the rewrap onto the line's first physical row"
        );
        assert_eq!(
            e.screen().scrollback_mark(1),
            None,
            "the wrapped continuation row carries no mark"
        );
        assert_eq!(
            e.screen().scrollback_rows().collect::<Vec<_>>(),
            ["abcd", "ef", "1"]
        );
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

    /// ⚠⚠ A NEEDLE THAT STRADDLES THE RIGHT EDGE IS FOUND — **R344 flipped the gap R343 measured
    /// and pinned here**, and this test kept its fixture so the two readings are comparable.
    ///
    /// A person reading a 20-column pane sees `abcdefghijklmnopqrstuvwxyz` on it. The emulator
    /// holds it as two rows and always knew they were one logical line; the search now walks
    /// LOGICAL lines ([`Screen::scan_logical`]) rather than physical rows, so the word the person
    /// is looking straight at is findable, and the answer says which rows it covers.
    ///
    /// The CONTROL is the second search: a needle inside one row is still one match with an empty
    /// `wrapped`, so the wrapping answer below is about the wrap and not about a search that now
    /// reports something for everything.
    ///
    /// ⚠ [`Screen::full_text`] still carries the row break, and that is deliberate: it is the
    /// RENDERED view — what the pane looks like — and a capture that silently rejoined lines would
    /// no longer describe the screen. The search is the surface that answers about CONTENT.
    #[test]
    fn a_needle_that_straddles_the_right_edge_is_found() {
        let e = em(20, 4, "abcdefghijklmnopqrstuvwxyz");
        let screen = e.screen();
        assert!(
            screen.wrapped(0),
            "the emulator knows row 0's logical line continues — the information is not missing",
        );
        assert_eq!(
            screen.find("abcdefghij").matches,
            vec![hit(0, 0, 10)],
            "THE CONTROL: a needle within one row is one span and wraps onto nothing",
        );
        assert_eq!(
            screen.find("abcdefghijklmnopqrstuvwxyz").matches,
            vec![FindMatch {
                line: 0,
                row: 0,
                col: 0,
                cols: 20,
                wrapped: vec![6],
            }],
            "the word on the screen is findable, and the answer says it covers 20 cells of row 0 \
             and the first 6 of row 1",
        );
        assert_eq!(
            screen.full_text(),
            "abcdefghijklmnopqrst\nuvwxyz",
            "the RENDERED view still carries the row break — it describes the screen, not the text",
        );
    }

    /// A needle can cross MORE than one margin, and each row it crosses is reported with the width
    /// it really covers. Three rows of a 5-column pane: the match starts mid-row, fills the rest of
    /// it, takes a whole row, and ends part-way through a third.
    #[test]
    fn a_needle_can_span_more_than_two_rows() {
        let e = em(5, 4, "ab123456789");
        let screen = e.screen();
        assert_eq!(
            screen.find("123456789").matches,
            vec![FindMatch {
                line: 0,
                row: 0,
                col: 2,
                cols: 3,
                wrapped: vec![5, 1],
            }],
            "3 cells on row 0 from column 2, all 5 of row 1, then 1 of row 2",
        );
    }

    /// A match that begins on a CONTINUATION row: its `row` is that row and its `line` is the row
    /// the LINE began on, which is the join key to the text. The two are different numbers here,
    /// and a shape that carried only one of them would have to choose between navigating to the
    /// match and finding its text.
    #[test]
    fn a_match_inside_a_continuation_row_still_names_its_line() {
        let e = em(5, 4, "abcdefghij");
        let screen = e.screen();
        assert_eq!(
            screen.find("gh").matches,
            vec![FindMatch {
                line: 0,
                row: 1,
                col: 1,
                cols: 2,
                wrapped: Vec::new(),
            }],
            "the match sits on row 1; the line it belongs to starts at row 0",
        );
        assert_eq!(
            screen.find("gh").lines,
            vec![FindLine {
                line: 0,
                text: "abcdefghij".to_owned(),
            }],
            "and the line entry is keyed on the line, carrying the WHOLE line's text",
        );
    }

    /// The join spans the scrollback→visible boundary, because a line half scrolled off is still
    /// one line. `reflowed` treats the two as one stream for the same reason; a search that did not
    /// would go blind at exactly the moment a long line starts scrolling away.
    #[test]
    fn a_needle_is_found_across_the_scrollback_boundary() {
        // 6 columns, 2 rows. "abcdefghij" wraps over two rows, then two more lines push the first
        // of them into scrollback.
        let e = em(6, 2, "abcdefghij\r\nkk\r\nll");
        let screen = e.screen();
        assert_eq!(
            screen.scrollback_len(),
            2,
            "the wrapped line's head scrolled off"
        );
        assert_eq!(
            screen.find("efgh").matches,
            vec![FindMatch {
                line: 0,
                row: 0,
                col: 4,
                cols: 2,
                wrapped: vec![2],
            }],
            "2 cells on the oldest scrollback row and 2 on the row after it",
        );
    }

    /// A line still OPEN at the last retained row is searched anyway — the guard that closes it,
    /// which was GREEN under mutation until this fixture existed.
    ///
    /// The traversal keeps a line open while its row says "continues", and the LAST retained row
    /// can say that: `DECSTBM` reserves the top rows as a scroll region and leaves the cursor
    /// parked on the row BELOW it (the status-line idiom), where printing past the margin sets the
    /// wrap flag and the line feed has nowhere to scroll to. Without the guard the accumulated
    /// line is never handed to the scan, and every match on the bottom row of such a screen
    /// silently disappears — measured here: 1 match becomes 0.
    ///
    /// The continuation itself points past the retained region, so there is nothing to join it to;
    /// the history encoder closes such a line for the same reason and says so in the same words
    /// ("the continuation is not ours to keep").
    #[test]
    fn a_line_still_open_at_the_last_retained_row_is_searched() {
        // Region = rows 1..2 (1-based), cursor parked on row 3, then 13 glyphs on a 10-column
        // screen: the wrap sets row 2's flag and the line feed cannot scroll it away.
        let e = em(10, 3, "\x1b[1;2r\x1b[3;1Habcdefghijklm");
        let screen = e.screen();
        assert!(
            screen.wrapped(screen.rows() - 1),
            "the fixture must leave the LAST row continuing onto a row that is not retained",
        );
        assert_eq!(
            screen.find("defghij").matches,
            vec![hit(2, 3, 7)],
            "the open line is closed and searched rather than dropped on the floor",
        );
    }

    /// A cell written PAST the column its row wrapped at is still findable.
    ///
    /// R344's own hazard, measured and closed in the same round: the search stopped reading a
    /// wrapped row at its wrap column, so a writer that went back and addressed a column beyond it
    /// — which nothing clears the flag for — put a character on the screen that the search could
    /// not see. `full_text` rendered `"ab世Z"` and `find("Z")` answered NOTHING.
    ///
    /// The wrap column EXTENDS on such a write rather than clearing, so the row keeps continuing
    /// onto the next: both halves are asserted here, because clearing would pass the first
    /// assertion and quietly split one logical line into two.
    #[test]
    fn a_cell_written_past_the_wrap_column_is_still_part_of_the_line() {
        // Row 0 wraps EARLY at column 3 (世 will not fit in column 4), leaving column 4 a pad.
        let mut e = em(5, 3, "ab\u{4e16}\u{4e16}");
        assert_eq!(e.screen().find("Z").matches.len(), 0, "nothing to find yet");
        // Address column 4 of row 0 directly and write there. Nothing clears the wrap flag.
        e.advance(b"\x1b[1;5HZ");
        let screen = e.screen();
        assert_eq!(
            screen.row_text(0),
            "ab\u{4e16}Z",
            "the fixture must put Z on the screen, past where the row wrapped",
        );
        assert_eq!(
            screen.find("Z").matches,
            vec![hit(0, 4, 1)],
            "a character a person can see is a character the search can find",
        );
        assert!(
            screen.wrapped(0),
            "and the row still continues onto the next: the write moved the column, not the wrap",
        );
        assert_eq!(
            screen.find("Z\u{4e16}").matches,
            vec![FindMatch {
                line: 0,
                row: 0,
                col: 4,
                cols: 1,
                wrapped: vec![2],
            }],
            "so the line still reads across the wrap, with Z now part of it",
        );
    }

    /// A HARD line break is not a wrap, and the join must not cross one — the negative control for
    /// the whole traversal. Without it "join the rows" would degrade into "search the pane as one
    /// string", and a needle spanning two unrelated lines would match.
    #[test]
    fn a_needle_does_not_span_a_hard_line_break() {
        let e = em(20, 4, "abcdefghij\r\nklmnopqrst");
        let screen = e.screen();
        assert!(!screen.wrapped(0), "the fixture must break HARD, not wrap");
        assert!(
            screen.find("abcdefghijklmnopqrst").matches.is_empty(),
            "two lines are two lines, however they look stacked on the screen",
        );
        assert_eq!(
            screen.find("klmno").matches,
            vec![hit(1, 0, 5)],
            "THE CONTROL: each line is searchable on its own",
        );
    }

    /// The blanks a wrapped row really holds are CONTENT, and the join keeps them: the child
    /// printed `"ab    cdef"` and the pane broke it at the margin, so the needle spanning the gap
    /// is there to be found. A join that trimmed each row (R343's drop-paste fold) would answer
    /// `"abcdef"` and miss it.
    #[test]
    fn the_join_keeps_the_blanks_a_wrapped_row_printed() {
        let e = em(6, 3, "ab    cdef");
        let screen = e.screen();
        assert_eq!(
            screen.find("b    c").matches,
            vec![FindMatch {
                line: 0,
                row: 0,
                col: 1,
                cols: 5,
                wrapped: vec![1],
            }],
            "five cells of row 0 from column 1, then one of row 1",
        );
    }

    /// ...and the pad an EARLY wrap left is NOT content, in the same join. A wide cluster that will
    /// not fit at the margin wraps a column early, leaving a blank the emulator never wrote; a join
    /// that took the whole row would put a space between the two clusters and the needle a person
    /// sees would not match. The other direction of
    /// [`the_join_keeps_the_blanks_a_wrapped_row_printed`] — one rule cannot satisfy both by
    /// accident, which is why both are here.
    #[test]
    fn the_join_drops_the_pad_an_early_wrap_left() {
        let e = em(5, 3, "ab\u{4e16}\u{4e16}");
        let screen = e.screen();
        assert_eq!(
            screen.row_text(0),
            "ab\u{4e16}",
            "the fixture must wrap EARLY: 世 is two columns and column 4 is the last",
        );
        assert_eq!(
            screen.find("\u{4e16}\u{4e16}").matches,
            vec![FindMatch {
                line: 0,
                row: 0,
                col: 2,
                cols: 2,
                wrapped: vec![2],
            }],
            "the two clusters are adjacent in the line: no pad between them, two columns each",
        );
        assert!(
            screen.find("\u{4e16} \u{4e16}").matches.is_empty(),
            "and the pad is not a space somebody could search for",
        );
    }

    /// The regex search walks the same logical lines — it is the same traversal, so a pattern
    /// crossing a wrap is found and reported in the same coordinate. Written because a fix applied
    /// to one of two searches is the shape this codebase keeps catching (`find` and `find_regex`
    /// share `scan_logical` precisely so they cannot disagree).
    #[test]
    fn a_pattern_spans_a_wrap_like_a_needle_does() {
        let e = em(5, 4, "abcdefgh");
        let screen = e.screen();
        assert_eq!(
            screen.find_regex("d.f").unwrap().matches,
            vec![FindMatch {
                line: 0,
                row: 0,
                col: 3,
                cols: 2,
                wrapped: vec![1],
            }],
            "`d.f` crosses the margin between 'e' and 'f'",
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

    /// Find spans the WHOLE retained output on ONE axis: scrollback lines first, then the visible
    /// grid, numbered from the oldest line — the same coordinate `prompt_positions` reports, which
    /// is what makes `scroll_to(match.line)` a legal jump.
    #[test]
    fn find_reports_matches_across_scrollback_and_the_visible_grid() {
        // 4 lines on a 2-row screen: "err a", "b" scroll off; "c", "err d" stay visible.
        let e = em(16, 2, "err a\r\nb\r\nc\r\nerr d");
        let screen = e.screen();
        assert_eq!(screen.scrollback_len(), 2, "two lines scrolled off");
        let found = screen.find("err");
        assert!(!found.truncated);
        assert_eq!(
            found.matches,
            vec![
                hit(0, 0, 3), // the oldest scrollback line
                hit(3, 0, 3), // visible row 1 = scrollback_len + 1
            ],
            "history and the live grid are ONE line axis, oldest first",
        );
    }

    /// Columns are CELLS, not bytes or chars. A wide cluster before a match shifts it by TWO
    /// columns, and a match ON one is two columns wide (its trailer is part of the highlight).
    /// REVERT-PROOF for the `starts` map + the trailer absorption: a byte-offset column would report
    /// `col: 3` for the ASCII match below, and dropping the trailer walk would report `cols: 1` for
    /// the wide one.
    /// The DISPLAY view: every matching line ONCE, with its text — what a grep-like consumer prints.
    /// Deduping is the point: the row below carries two matches and must still be one line.
    #[test]
    fn find_reports_each_matching_line_once_with_its_text() {
        let e = em(16, 2, "err a err\r\nquiet\r\nerr b");
        let found = e.screen().find("err");
        assert_eq!(
            found.matches.len(),
            3,
            "two on the first line, one on the last"
        );
        assert_eq!(
            found.lines,
            vec![
                FindLine {
                    line: 0,
                    text: "err a err".to_owned()
                },
                FindLine {
                    line: 2,
                    text: "err b".to_owned()
                },
            ],
            "one entry per matching LINE, in order, with the untouched original text",
        );
    }

    #[test]
    fn find_columns_are_cells_not_bytes_for_a_wide_cluster() {
        let e = em(16, 2, "x\u{ac00}y err");
        let screen = e.screen();
        assert_eq!(
            screen.find("err").matches,
            vec![hit(0, 5, 3)],
            "x=0, the wide cluster=1..2, y=3, space=4 -> the match starts at column 5",
        );
        assert_eq!(
            screen.find("\u{ac00}").matches,
            vec![hit(0, 1, 2)],
            "a wide cluster occupies two columns, trailer included",
        );
    }

    /// ASCII case folding both ways, and a non-overlapping scan. Both are contracts a find bar's
    /// match COUNT depends on, so they are pinned rather than left to `str::find`'s defaults.
    #[test]
    fn find_is_ascii_case_insensitive_and_non_overlapping() {
        let e = em(16, 1, "ERror aaa");
        let screen = e.screen();
        assert_eq!(
            screen.find("error").matches.len(),
            1,
            "needle case is folded"
        );
        assert_eq!(
            screen.find("ERROR").matches.len(),
            1,
            "haystack case is folded"
        );
        assert_eq!(
            screen.find("aa").matches,
            vec![hit(0, 6, 2)],
            "`aa` occurs ONCE in `aaa` — the scan resumes past a match, never inside it",
        );
    }

    /// The blanks a grid pads every row with are not content. REVERT-PROOF for the `trim_end` bound:
    /// without it a two-space needle would match the filler on every row of the screen.
    #[test]
    fn find_ignores_the_grids_trailing_padding() {
        let e = em(16, 2, "a  b");
        let screen = e.screen();
        assert_eq!(
            screen.find("  ").matches,
            vec![hit(0, 1, 2)],
            "only the interior gap matches, never the padding out to `cols`",
        );
    }

    /// A search is BOUNDED, and says so. A one-character needle over a full scrollback can match
    /// more times than any consumer can use, so the scan stops at the cap and reports it rather than
    /// answering a silently partial list that a match counter would then misdraw.
    #[test]
    fn find_caps_its_answer_and_reports_the_truncation() {
        let line = "a".repeat(80);
        let feed: String = std::iter::repeat_n(line.as_str(), 20)
            .collect::<Vec<_>>()
            .join("\r\n");
        let e = em(80, 2, &feed);
        let found = e.screen().find("a");
        assert_eq!(
            found.matches.len(),
            FIND_MATCH_CAP,
            "the scan stops at the cap"
        );
        assert!(found.truncated, "and the answer admits it is capped");
    }

    /// An empty needle is not a match at every position — it is nothing to look for.
    #[test]
    fn find_of_an_empty_needle_matches_nothing() {
        let e = em(16, 1, "anything");
        let found = e.screen().find("");
        assert!(found.matches.is_empty() && !found.truncated);
    }

    /// The regex search answers on the SAME axis and in the same coordinates as the literal one —
    /// scrollback first, then the visible grid — so a client can jump to and highlight a regex hit
    /// with the machinery it already has.
    #[test]
    fn find_regex_reports_matches_across_scrollback_and_the_visible_grid() {
        let e = em(16, 2, "err a\r\nb\r\nc\r\nerr d");
        let screen = e.screen();
        assert_eq!(screen.scrollback_len(), 2, "two lines scrolled off");
        let found = screen.find_regex("err .").expect("a valid pattern");
        assert_eq!(
            found
                .matches
                .iter()
                .map(|m| (m.line, m.col, m.cols))
                .collect::<Vec<_>>(),
            vec![(0, 0, 5), (3, 0, 5)],
            "one in scrollback, one visible, both in cell columns",
        );
        assert_eq!(
            found
                .lines
                .iter()
                .map(|l| l.text.as_str())
                .collect::<Vec<_>>(),
            vec!["err a", "err d"],
        );
    }

    /// The two searches are different LANGUAGES over the same input, which is why they are
    /// different entries rather than one with a mode: `a.b` is three literal characters to `find`
    /// and "a, anything, b" to `find_regex`. Neither reading is wrong; reading the caller's string
    /// in the language they did not pick would be.
    #[test]
    fn a_needle_and_a_pattern_read_the_same_string_differently() {
        let e = em(16, 1, "axb a.b");
        let screen = e.screen();
        assert_eq!(
            screen.find("a.b").matches.len(),
            1,
            "literally, only the real dot matches",
        );
        assert_eq!(
            screen.find_regex("a.b").expect("valid").matches.len(),
            2,
            "as a pattern, the dot matches any character",
        );
    }

    /// Case is the pattern language's to decide: `find_regex` is case-SENSITIVE (unlike `find`), and
    /// `(?i)` is how a caller asks for folding — so the flag they wrote is never overruled.
    #[test]
    fn find_regex_is_case_sensitive_until_the_pattern_says_otherwise() {
        let e = em(16, 1, "Error error");
        let screen = e.screen();
        assert_eq!(screen.find_regex("Error").expect("valid").matches.len(), 1);
        assert_eq!(
            screen.find_regex("(?i)error").expect("valid").matches.len(),
            2,
            "(?i) is the caller's own switch",
        );
        assert_eq!(
            screen.find("error").matches.len(),
            2,
            "the literal search folds ASCII case by contrast",
        );
    }

    /// Columns are CELLS, not bytes, for the regex search too — the mapping both searches share.
    /// A wide cluster is one cluster and TWO columns, so a byte offset could not be a column.
    #[test]
    fn find_regex_columns_are_cells_not_bytes_for_a_wide_cluster() {
        // "가" is one cluster occupying columns 0-1; "ab" then sits at columns 2 and 3.
        let e = em(16, 1, "가ab");
        let found = e.screen().find_regex("a.").expect("valid");
        assert_eq!(
            found
                .matches
                .iter()
                .map(|m| (m.col, m.cols))
                .collect::<Vec<_>>(),
            vec![(2, 2)],
            "the match starts at CELL 2, not byte 3",
        );
    }

    /// A pattern the engine refuses answers with its OWN explanation, not a bare "no matches" —
    /// which is the difference between "your pattern is wrong here" and "your search found nothing".
    #[test]
    fn an_invalid_pattern_is_refused_with_the_engines_message() {
        let e = em(16, 1, "anything");
        let refused = e
            .screen()
            .find_regex("a(b")
            .expect_err("an unclosed group is refused");
        assert!(
            refused.message().contains("("),
            "the message points at the pattern: {refused}",
        );
    }

    /// A pathological pattern is refused by OUR compile bound rather than built on the interactive
    /// path. Self-discriminating: the same pattern compiles fine under the engine's much larger
    /// default, so what refuses it here can only be [`REGEX_SIZE_LIMIT`].
    #[test]
    fn an_oversized_pattern_is_refused_by_our_compile_bound() {
        // Nested bounded repetition: ~90k states, well past our limit and well inside the default.
        let pattern = "(?:a{300}){300}";
        let e = em(16, 1, "anything");
        let refused = e
            .screen()
            .find_regex(pattern)
            .expect_err("our bound refuses it");
        assert!(
            refused.message().contains("size limit"),
            "refused for SIZE, not syntax: {refused}",
        );
        assert!(
            regex::Regex::new(pattern).is_ok(),
            "…yet the engine's own default would have built it, so the bound is ours",
        );
    }

    /// A pattern that can match EMPTY terminates and reports nothing for the empty hits: a
    /// zero-width match covers no cells, so there is no column for a coordinate to point at.
    #[test]
    fn a_zero_width_match_is_not_reported_and_does_not_loop() {
        let e = em(16, 1, "aaa bbb");
        let found = e.screen().find_regex("x*").expect("valid");
        assert!(
            found.matches.is_empty(),
            "nothing to highlight: {:?}",
            found.matches,
        );
        // The same pattern with a non-empty alternative still reports the real hits.
        let mixed = e.screen().find_regex("b*").expect("valid");
        assert_eq!(mixed.matches.len(), 1, "only the non-empty run: {mixed:?}");
    }

    /// An empty pattern mirrors an empty needle: nothing to look for, not everything.
    #[test]
    fn find_regex_of_an_empty_pattern_matches_nothing() {
        let e = em(16, 1, "anything");
        let found = e.screen().find_regex("").expect("empty is not an error");
        assert!(found.matches.is_empty() && !found.truncated);
    }

    /// The grid's trailing padding is outside the search for the regex too — otherwise `$` would
    /// anchor at the right MARGIN rather than at the end of the line's content.
    #[test]
    fn find_regex_anchors_at_the_content_end_not_the_margin() {
        let e = em(16, 1, "short");
        assert_eq!(
            e.screen()
                .find_regex("short$")
                .expect("valid")
                .matches
                .len(),
            1,
            "`$` sits after the last glyph, not after the padding",
        );
    }
    /// ⚠ **A SELECTION SPELLS ITSELF ONCE, and the OSC character is DERIVED from that spelling.**
    ///
    /// [`ClipboardTarget::osc_char`] used to be its own `match`, which was harmless while nothing else
    /// spelled the words — and stopped being harmless when the pane surface came to PUBLISH them (the
    /// `clipboard_answer` action's `sel`, and the pending query's `sel` in the pane list, which are
    /// the same vocabulary read in two directions). Two matches over two words is how a reply comes to
    /// echo one character while the wire admits another.
    ///
    /// So the claim is the derivation: over the whole type, the OSC character IS the wire word's one
    /// character, and the word is exactly one character long — the property that makes deriving it
    /// possible at all.
    #[test]
    fn a_selections_osc_character_is_its_wire_word() {
        for target in ClipboardTarget::ALL {
            let word = target.wire_str();
            assert_eq!(
                word.chars().count(),
                1,
                "{word:?} is an OSC 52 selection character, so it is one character",
            );
            assert_eq!(
                target.osc_char(),
                word.chars().next().expect("one character")
            );
            assert_eq!(ClipboardTarget::from_wire(word), Some(target));
        }
        assert_eq!(ClipboardTarget::WIRE_WORDS, ["c", "p"]);
        // The OSC 52 `Pc` parse is DELIBERATELY more lenient than this vocabulary (an `s` or an empty
        // field from a CHILD folds onto the clipboard), and that leniency is about programs a terminal
        // must tolerate — not about a client of sprag's own wire, which is told two words and held to
        // them.
        for stranger in ["s", "", "0", "C", "cp"] {
            assert_eq!(ClipboardTarget::from_wire(stranger), None, "{stranger:?}");
        }
    }
}
