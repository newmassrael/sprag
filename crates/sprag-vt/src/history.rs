//! Encoding a pane's retained output as REPLAYABLE terminal bytes — the durable form of
//! scrollback.
//!
//! The durability ring can carry a pane's shape across a reboot but not its PTY, so a restored
//! pane comes back blank: its history — the thing a user scrolls back through and the thing
//! [`Screen::find`](crate::port::Screen::find) searches — dies with the daemon. This module is the
//! serialization that fixes that.
//!
//! ## Why the durable form is terminal bytes, not a cell struct
//!
//! A [`Cell`] carries a cluster, three colour channels, eight attributes, a width role and an
//! OSC-8 link. Serializing that struct would mean inventing a second encoding for it — a schema to
//! version, a deserializer to keep in step with the emulator, and a format no other tool reads.
//! But a faithful encoding of a styled cell run ALREADY EXISTS and the emulator already parses it:
//! SGR. So history is stored as the terminal's own language, and the "deserializer" is the
//! emulator itself — replay the bytes into a fresh [`Emulator`](crate::Emulator) at the recorded
//! width and the cells come back. There is one encoder here and no decoder anywhere, which is why
//! [`round-trip`](self) is a property this module can actually assert.
//!
//! Two consequences worth naming. The file on disk is `cat`-able (it IS terminal output). And the
//! restored pane reconstructs its own soft wrapping: a logical line is emitted as one continuous
//! run and the emulator's autowrap re-breaks it, so the wrap structure is DERIVED at the recorded
//! width rather than transcribed — the same structure a live pane would have had.
//!
//! ## The generated alphabet is closed
//!
//! Replaying recorded bytes through an emulator is only safe because sprag GENERATES them: this
//! encoder emits SGR (colour / attributes), OSC 8 (hyperlinks), OSC 133 (prompt marks), Kitty
//! graphics transmits (`APC _G a=T`, for inline images), printable clusters and `CR LF` — nothing
//! else. No cursor positioning, no scroll regions, no mode changes, no DCS. Anything the child once
//! sent that this encoder does not emit is not in the file, so a replay cannot re-run it. The cells
//! are the input, not the child's original byte stream.
//!
//! ## Inline images are encoded, in the same language
//!
//! A Kitty / Sixel image is re-emitted as a Kitty `a=T` transmit at its anchor cell — an image raster
//! is not a cell, but it IS something the terminal's own language can express, so it needs no second
//! file, no second lifecycle and (the load-bearing part) no second decoder. A Sixel image comes back
//! as Kitty bytes: the durable form is the emulator's model of the image, not the syntax the child
//! happened to use to transmit it, exactly as a styled cell comes back as SGR rather than as whatever
//! the child originally sent.
//!
//! Bounded by [`HistoryLimits::image_bytes`], because a raster is orders of magnitude larger than the
//! text beside it and a screen may hold hundreds of them.
//!
//! ## What is deliberately NOT encoded (documented bounds)
//!
//! * **The DECSCA protection bit** ([`Cell::protected`]) — invisible by definition and
//!   emulator-internal (it never reaches the wire either). A restored cell comes back unprotected;
//!   the selective-erase family that reads it is a live-app concern, not a history one.
//! * **The alternate screen** — see [`Emulator::history_bytes`](crate::Emulator::history_bytes):
//!   a fullscreen app's buffer is furniture its own program redraws.
//! * **Where the cursor sat inside the last line** — history ends at a line boundary, so a
//!   restored pane's own first output starts on a fresh row rather than overwriting the last
//!   recorded line.
//! * **The absolute row, when the content exactly FILLS the screen** — that last line boundary is a
//!   newline, and on a full screen a newline scrolls, so everything (text and images alike) comes back
//!   one row higher with the top line in scrollback. Content and relative placement are preserved;
//!   only the absolute row moves. Guarded by
//!   `a_screen_filling_history_scrolls_its_image_up_with_its_text`.

use std::sync::Arc;

use crate::port::{Attrs, Cell, Color, Hyperlink, Image, PromptMark, UnderlineStyle, Width};

/// One retained row handed to [`encode`]: its cells and whether its logical line CONTINUES onto
/// the next row (the soft-wrap flag).
///
/// A borrowed view rather than an owned row, so assembling the retained region
/// ([`Screen::history_bytes`](crate::port::Screen::history_bytes)) clones no cells — scrollback
/// and the visible grid are both already `[Cell]` runs.
pub(crate) struct HistoryRow<'a> {
    pub(crate) cells: &'a [Cell],
    pub(crate) wrapped: bool,
    /// The shell-integration mark this row carries, if any. Emitted only for a logical line's
    /// FIRST row (see [`encode`]).
    pub(crate) mark: Option<PromptMark>,
    /// The inline images ANCHORED on this row, each with the column it sits at, in transmit order.
    /// Empty for a scrollback row — an image lives on the visible grid only.
    pub(crate) images: &'a [(u16, &'a Image)],
}

/// How much of a pane's history to encode, on the two axes it has.
///
/// A struct rather than two arguments because the axes have accrued (lines, now image bytes) and the
/// next bound should be a field rather than a churned call site — the `PaneRebirth` lesson from the
/// restore path, applied before the third argument rather than after it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct HistoryLimits {
    /// How many LOGICAL lines of text to keep. `0` encodes nothing at all — the "persistence is off"
    /// answer, on both axes.
    pub lines: usize,
    /// How many bytes of RASTER (pre-base64 RGBA) to keep across all of a pane's images. `0` keeps the
    /// text and drops every image.
    ///
    /// A budget is not optional: a screen may hold [`IMAGE_CAP`](crate::port) images of
    /// `MAX_IMAGE_BYTES` each, so an unbounded encoder could turn one save tick into gigabytes of
    /// base64. Images are taken in TRANSMIT order until the budget is spent, so the oldest survive a
    /// restore — the same direction scrollback retention takes, and the one a user can predict.
    pub image_bytes: usize,
}

impl HistoryLimits {
    /// Limits that encode nothing — the disabled answer.
    #[must_use]
    pub const fn none() -> Self {
        Self {
            lines: 0,
            image_bytes: 0,
        }
    }

    /// `lines` of text with no images — the text-only encoding.
    #[must_use]
    pub const fn text_only(lines: usize) -> Self {
        Self {
            lines,
            image_bytes: 0,
        }
    }
}

/// Encode the last `limit` LOGICAL lines of `rows` as replayable terminal bytes.
///
/// Logical lines, not physical rows, are the unit — the same width-independent unit
/// [`SCROLLBACK_CAP`](crate::port) bounds retention by. A soft-wrapped line is emitted as ONE
/// continuous run of clusters with a single trailing `CR LF`, so the replaying emulator re-breaks
/// it by autowrap at whatever width it is replayed into.
///
/// Per logical line: the first row's [`PromptMark`] (if any) is emitted first, then every row's
/// clusters — verbatim for the continuation rows (their full width is what puts the next
/// character in the right column) and trailing-blank-trimmed for the LAST row, so a line does not
/// come back padded to the margin. Wide-cluster trailers emit nothing; the wide head re-creates
/// its own trailer on replay. Style is emitted as a DELTA: an SGR reset plus the cell's non-default
/// components, once per style change rather than once per cell.
///
/// A mark on a CONTINUATION row is dropped rather than emitted mid-line: at the moment a full row
/// ends the cursor is in the deferred-wrap state (still on the row it filled), so an escape emitted
/// there would attach to the previous row. Reflow already re-attaches a logical line's mark to its
/// first physical row, so first-row-only matches the model the rest of the emulator keeps.
///
/// ## Inline images
///
/// An image is emitted as a Kitty transmit-and-display (`a=T`) sequence AT THE POINT IN THE STREAM
/// WHERE ITS ANCHOR CELL IS WRITTEN, so the replaying emulator anchors it exactly where it was: the
/// transmit places at the cursor and does not move it, which is what lets it sit between two cells of
/// a row. Nothing positions the cursor explicitly — placement is derived from the same walk that
/// derives the text, so it cannot disagree with where the characters landed. (Emitting the images
/// after the text with cursor addressing would depend on the replay's final layout, which the
/// trailing-blank trim and the line limit both perturb.)
///
/// An anchor to the RIGHT of a row's last glyph is reached by padding with spaces. That is
/// content-neutral: [`Cell::blank`] IS a space with a default pen, so the padded cells compare equal
/// to the blanks the trim removed.
///
/// `limits.lines == 0` encodes nothing — the "history persistence is off" answer.
pub(crate) fn encode(rows: &[HistoryRow<'_>], limits: HistoryLimits) -> Vec<u8> {
    let lines = logical_lines(rows);
    let start = lines.len().saturating_sub(limits.lines);
    let mut out = Vec::new();
    let mut pen = Pen::default();
    let mut image_budget = limits.image_bytes;
    for line in &lines[start..] {
        let (first, last) = *line;
        if let Some(mark) = rows[first].mark {
            write_mark(&mut out, mark);
        }
        for (index, row) in rows[first..=last].iter().enumerate() {
            // The last row of a logical line ends it, so its padding to the right margin is not
            // content; a continuation row's full width IS content (it positions the next row).
            let cells = if first + index == last {
                trim_trailing_blanks(row.cells)
            } else {
                row.cells
            };
            let mut col = 0u16;
            let mut pending = row.images;
            for cell in cells {
                (pending, col) =
                    write_images_at(&mut out, pending, col, col, &mut pen, &mut image_budget);
                // A wide cluster's trailer holds no glyph of its own (an empty cluster) and copies
                // its head's style, so emitting it would add no bytes — the head re-creates the
                // trailer on replay by being two columns wide. Skipping it is an optimization, not
                // a correctness guard: the encoding is byte-identical either way.
                if cell.width == Width::Trailer {
                    col = col.saturating_add(1);
                    continue;
                }
                pen.apply(&mut out, cell);
                out.extend_from_slice(cell.cluster.as_bytes());
                // A wide head occupies TWO columns, so the anchor column of anything after it must
                // count both — the cursor advanced by two.
                col = col.saturating_add(if cell.width == Width::Wide { 2 } else { 1 });
            }
            // Any image anchored PAST the row's last glyph: pad to its column and place it there.
            let far = pending.last().map_or(col, |(anchor, _)| *anchor);
            (_, _) = write_images_at(&mut out, pending, col, far, &mut pen, &mut image_budget);
        }
        out.extend_from_slice(b"\r\n");
    }
    // Leave the terminal as we found it, so the restored pane's own first output (a fresh shell's
    // prompt) starts with a default pen and no open hyperlink rather than inheriting history's.
    pen.reset(&mut out);
    out
}

/// Emit every image in `pending` anchored at a column in `[col, through]`, padding with spaces from
/// `col` to each anchor, and return the images still ahead plus the column the cursor now sits at.
///
/// Splitting this out is what lets the two callers — mid-row (place before the cell about to be
/// written) and end-of-row (place past the last glyph) — share ONE placement rule, so an image cannot
/// be positioned one way inside a row and another way at its end.
fn write_images_at<'a>(
    out: &mut Vec<u8>,
    pending: &'a [(u16, &'a Image)],
    mut col: u16,
    through: u16,
    pen: &mut Pen,
    budget: &mut usize,
) -> (&'a [(u16, &'a Image)], u16) {
    let mut rest = pending;
    while let Some(((anchor, image), tail)) = rest.split_first() {
        if *anchor > through {
            break;
        }
        let raster = image.rgba.len();
        if raster > *budget {
            // Out of budget: this image and every later one are dropped. Keep walking rather than
            // breaking, so a small image after a large one is not blocked by it.
            rest = tail;
            continue;
        }
        // The pen must be at its default before the padding, or the spaces would carry the previous
        // cell's background and paint a coloured gap the original never had.
        while col < *anchor {
            pen.apply(out, &Cell::blank());
            out.push(b' ');
            col = col.saturating_add(1);
        }
        write_kitty_image(out, image);
        *budget -= raster;
        rest = tail;
    }
    (rest, col)
}

/// The base64 chunk size for a Kitty transmit, in ENCODED bytes — kitty's own documented maximum for
/// one escape sequence's payload, and a multiple of 4 so no chunk splits a base64 quantum.
///
/// Chunked rather than one giant sequence because that is what the protocol specifies for a payload of
/// any size, and the emulator that will replay these bytes implements the chunked path (`m=1`) with
/// its own accumulator bound. A single unbounded APC sequence would rest on the parser tolerating an
/// arbitrarily long escape, which the protocol never promises.
const KITTY_CHUNK: usize = 4096;

/// Write one image as a Kitty transmit-and-display sequence: RGBA (`f=32`) at its own pixel
/// dimensions, under its own image id, chunked per [`KITTY_CHUNK`].
///
/// `a=T` displays at the cursor, which the caller has positioned at the anchor cell. The full control
/// data rides the FIRST chunk only (the continuation chunks carry `m=` and payload), matching both the
/// protocol and the emulator's accumulator, which reads format / size / id / display from the chunk
/// that opened the transmission.
fn write_kitty_image(out: &mut Vec<u8>, image: &Image) {
    use base64::Engine as _;
    let payload = base64::engine::general_purpose::STANDARD.encode(&image.rgba);
    let mut chunks = payload.as_bytes().chunks(KITTY_CHUNK).peekable();
    let mut first = true;
    while let Some(chunk) = chunks.next() {
        let more = u8::from(chunks.peek().is_some());
        out.extend_from_slice(b"\x1b_G");
        if first {
            // `f=32` = RGBA; `s`/`v` = the raster's pixel width/height; `i` = the image id, so a
            // restored image keeps the identity a later re-transmit would replace.
            out.extend_from_slice(
                format!(
                    "a=T,f=32,s={},v={},i={},m={more}",
                    image.width, image.height, image.id
                )
                .as_bytes(),
            );
            first = false;
        } else {
            out.extend_from_slice(format!("m={more}").as_bytes());
        }
        out.push(b';');
        out.extend_from_slice(chunk);
        out.extend_from_slice(b"\x1b\\");
    }
}

/// The `(first, last)` row index of each logical line in `rows`, in order.
///
/// A row STARTS a logical line when it is the first row or its predecessor did not soft-wrap. The
/// final line may end on a row whose `wrapped` flag is set — a logical line continuing past the
/// retained region — and is closed anyway; the continuation is not ours to keep.
fn logical_lines(rows: &[HistoryRow<'_>]) -> Vec<(usize, usize)> {
    let mut lines = Vec::new();
    let mut start = 0usize;
    for (index, row) in rows.iter().enumerate() {
        if !row.wrapped {
            lines.push((start, index));
            start = index + 1;
        }
    }
    if start < rows.len() {
        lines.push((start, rows.len() - 1));
    }
    lines
}

/// `cells` without its trailing run of cells that are indistinguishable from a blank — the grid
/// padding between a line's last glyph and the right margin.
///
/// "Blank" is full [`Cell`] equality, not a space cluster: a trailing run carrying a BACKGROUND
/// colour is a coloured bar the user can see, so it is content and stays.
fn trim_trailing_blanks(cells: &[Cell]) -> &[Cell] {
    let blank = Cell::blank();
    let end = cells.iter().rposition(|cell| *cell != blank);
    match end {
        Some(last) => &cells[..=last],
        None => &[],
    }
}

/// The style axes one SGR run carries — everything [`Cell`] renders with except its cluster,
/// width role and hyperlink (which is OSC 8, not SGR).
#[derive(Clone, Copy, PartialEq, Eq, Default)]
struct Style {
    fg: Color,
    bg: Color,
    underline_color: Option<Color>,
    attrs: Attrs,
}

impl Style {
    fn of(cell: &Cell) -> Self {
        Self {
            fg: cell.fg,
            bg: cell.bg,
            // `Some(Color::Default)` is the SGR-59 default spelled the long way; normalize it so
            // it does not read as a style CHANGE against a `None` pen (nothing is emitted for
            // either, and the emulator produces only `None` for SGR 59).
            underline_color: cell.underline_color.filter(|c| *c != Color::Default),
            attrs: cell.attrs,
        }
    }
}

/// The encoder's running terminal state: what the last emitted escape left the style and the open
/// hyperlink at. Emitting only the DELTA is what keeps a styled line from carrying an escape per
/// cell.
#[derive(Default)]
struct Pen {
    /// The style the emitted bytes have established, or `None` before anything is emitted (no
    /// assumption is made about the replaying terminal's initial pen — the first cell emits a
    /// reset).
    style: Option<Style>,
    /// The hyperlink the emitted bytes have left OPEN, by `Arc` identity. Identity rather than URI
    /// equality because two anonymous links to the same URI are two distinct runs
    /// ([`Hyperlink`]) — comparing values would silently merge them into one.
    link: Option<Arc<Hyperlink>>,
}

impl Pen {
    /// Emit whatever escapes are needed for `cell` to render correctly, and record them.
    fn apply(&mut self, out: &mut Vec<u8>, cell: &Cell) {
        let style = Style::of(cell);
        if self.style != Some(style) {
            write_sgr(out, &style);
            self.style = Some(style);
        }
        let same_link = match (&self.link, &cell.hyperlink) {
            (None, None) => true,
            (Some(open), Some(want)) => Arc::ptr_eq(open, want),
            _ => false,
        };
        if !same_link {
            write_link(out, cell.hyperlink.as_deref());
            self.link = cell.hyperlink.clone();
        }
    }

    /// Close anything left open: a default pen and no hyperlink.
    fn reset(&mut self, out: &mut Vec<u8>) {
        if self.link.is_some() {
            write_link(out, None);
            self.link = None;
        }
        if self.style.is_some_and(|s| s != Style::default()) {
            out.extend_from_slice(b"\x1b[0m");
            self.style = Some(Style::default());
        }
    }
}

/// Emit `style` as an SGR reset followed by its non-default components.
///
/// Reset-then-set rather than a per-axis diff: SGR 0 is the one code that clears EVERY axis at
/// once, so what follows depends on nothing about the previous state. A diff would have to emit
/// the "turn this off" code for each axis independently (and SGR has no unambiguous off-code for
/// bold-vs-dim — 22 clears both), which is how a delta encoder silently leaks state.
fn write_sgr(out: &mut Vec<u8>, style: &Style) {
    out.extend_from_slice(b"\x1b[0m");
    let Attrs {
        bold,
        dim,
        italic,
        underline,
        blink,
        reverse,
        hidden,
        strikethrough,
    } = style.attrs;
    if bold {
        out.extend_from_slice(b"\x1b[1m");
    }
    if dim {
        out.extend_from_slice(b"\x1b[2m");
    }
    if italic {
        out.extend_from_slice(b"\x1b[3m");
    }
    // The 4:x subparameter spelling for EVERY underline style, including single (4:1) and double
    // (4:2). The legacy spellings are ambiguous — SGR 21 is "double underline" here but "bold off"
    // elsewhere — and 4:x is the one form that names the style axis unambiguously.
    let underline = match underline {
        UnderlineStyle::None => None,
        UnderlineStyle::Single => Some(1),
        UnderlineStyle::Double => Some(2),
        UnderlineStyle::Curly => Some(3),
        UnderlineStyle::Dotted => Some(4),
        UnderlineStyle::Dashed => Some(5),
    };
    if let Some(style) = underline {
        out.extend_from_slice(format!("\x1b[4:{style}m").as_bytes());
    }
    if blink {
        out.extend_from_slice(b"\x1b[5m");
    }
    if reverse {
        out.extend_from_slice(b"\x1b[7m");
    }
    if hidden {
        out.extend_from_slice(b"\x1b[8m");
    }
    if strikethrough {
        out.extend_from_slice(b"\x1b[9m");
    }
    write_color(out, style.fg, 38);
    write_color(out, style.bg, 48);
    if let Some(color) = style.underline_color {
        write_color(out, color, 58);
    }
}

/// Emit one colour channel — `lead` is 38 (foreground), 48 (background) or 58 (underline), the
/// three that share the `;5;<index>` / `;2;<r>;<g>;<b>` grammar. [`Color::Default`] emits nothing:
/// the SGR reset already left every channel at its default.
fn write_color(out: &mut Vec<u8>, color: Color, lead: u8) {
    let sgr = match color {
        Color::Default => return,
        Color::Indexed(index) => format!("\x1b[{lead};5;{index}m"),
        Color::Rgb(rgb) => format!("\x1b[{lead};2;{};{};{}m", rgb.r, rgb.g, rgb.b),
    };
    out.extend_from_slice(sgr.as_bytes());
}

/// Open `link` as an OSC 8 hyperlink, or close the open one when `None`.
///
/// Terminated with ST (`ESC \`) rather than BEL: both are accepted, but ST is the form the
/// standard specifies and it cannot be confused with a literal bell in a `cat`-ed history file.
fn write_link(out: &mut Vec<u8>, link: Option<&Hyperlink>) {
    let osc = match link {
        Some(link) => {
            let id = link.id.as_deref().unwrap_or_default();
            let params = if id.is_empty() {
                String::new()
            } else {
                format!("id={id}")
            };
            format!("\x1b]8;{params};{}\x1b\\", link.uri)
        }
        None => "\x1b]8;;\x1b\\".to_owned(),
    };
    out.extend_from_slice(osc.as_bytes());
}

/// Emit a shell-integration boundary mark (OSC 133), so a restored pane keeps its jump-to-prompt
/// targets and its last command's exit status.
fn write_mark(out: &mut Vec<u8>, mark: PromptMark) {
    let osc = match mark {
        PromptMark::Prompt => "\x1b]133;A\x1b\\".to_owned(),
        PromptMark::Output => "\x1b]133;C\x1b\\".to_owned(),
        PromptMark::CommandEnd(None) => "\x1b]133;D\x1b\\".to_owned(),
        PromptMark::CommandEnd(Some(exit)) => format!("\x1b]133;D;{exit}\x1b\\"),
    };
    out.extend_from_slice(osc.as_bytes());
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::emulator::Emulator;
    use crate::port::{Screen, VtPort};

    /// A pane's retained rows as CONTENT: scrollback then the visible grid, each row trimmed of
    /// its right-margin padding, trailing empty rows dropped.
    ///
    /// The comparison unit for the round-trip, and deliberately NOT the scrollback/visible SPLIT:
    /// where the boundary falls is a function of how much output a screen of that height has
    /// taken, and a replay that has just been fed its whole history has taken it all at once. The
    /// durable claim is that the retained CONTENT comes back, cell for cell.
    fn retained(screen: &Screen) -> Vec<Vec<Cell>> {
        let mut rows: Vec<Vec<Cell>> = screen
            .scrollback_cells()
            .map(|cells| trim_trailing_blanks(cells).to_vec())
            .collect();
        for row in 0..screen.rows() {
            rows.push(trim_trailing_blanks(&screen.row_cells(row)).to_vec());
        }
        while rows.last().is_some_and(Vec::is_empty) {
            rows.pop();
        }
        rows
    }

    /// Feed `bytes` to a fresh `cols x rows` emulator.
    fn emulate(cols: u16, rows: u16, bytes: &[u8]) -> Emulator {
        let mut em = Emulator::new(cols, rows);
        em.advance(bytes);
        em
    }

    /// Every style axis a [`Cell`] carries, over a screen small enough that most of it has already
    /// scrolled into history — so the fixture exercises the scrollback and visible paths at once.
    fn styled_fixture() -> Emulator {
        emulate(
            20,
            4,
            b"plain\r\n\
              \x1b[1;31;44mbold red on blue\x1b[0m\r\n\
              \x1b[3;4:3m\x1b[58;2;255;0;0mitalic curly\x1b[0m\r\n\
              \x1b[38;2;10;20;30;7;9mtruecolor rev struck\x1b[0m\r\n\
              \x1b]8;;https://example.com\x1b\\LINK\x1b]8;;\x1b\\ after\r\n\
              wide \xed\x95\x9c\xea\xb8\x80 cluster\r\n\
              \x1b[2;5mdim blink\x1b[0m\r\n\
              this line is long enough to soft wrap past twenty columns\r\n\
              tail\r\n",
        )
    }

    /// THE round-trip invariant, and the reason terminal bytes are the durable form: encoding a
    /// pane's history and replaying it into a fresh emulator of the same width reconstructs the
    /// retained cells EXACTLY — clusters, all three colour channels, every attribute, the
    /// underline style axis, wide-cluster width roles and OSC-8 link targets.
    ///
    /// This is the property that lets the module ship one encoder and NO decoder: the emulator
    /// already is the decoder, so there is no second implementation to drift.
    #[test]
    fn encoded_history_replays_into_identical_cells() {
        let original = styled_fixture();
        let bytes = original.history_bytes(HistoryLimits::text_only(1000));
        let replayed = emulate(20, 4, &bytes);

        assert_eq!(
            retained(replayed.screen()),
            retained(original.screen()),
            "the replayed pane's retained cells differ from the encoded ones",
        );
        // Non-vacuous: the fixture really did carry style, not just text.
        let styled = retained(original.screen())
            .iter()
            .flatten()
            .any(|cell| cell.attrs != Attrs::default() || cell.fg != Color::Default);
        assert!(styled, "the fixture must exercise styled cells");
    }

    /// THE image round-trip: an encoded history replays into IDENTICAL images — same pixels, same id,
    /// same anchor CELL — while the text around them comes back unchanged.
    ///
    /// This is the property that makes the images half assertable at all, and it holds for the same
    /// reason the cells half does: there is one encoder and NO decoder. The bytes written are Kitty
    /// transmit sequences, and the thing that reads them back is the emulator itself, so a drift
    /// between writer and reader is not expressible.
    ///
    /// The fixture puts images in the three positions that are decided by different code: at column 0
    /// (before any cell of the row), MID-ROW (between two cells — the case that needs the transmit not
    /// to move the cursor), and PAST the row's last glyph (the case that needs space padding to reach
    /// the anchor). A single image would have exercised only one of them.
    ///
    /// REVERT-PROOF: emitting every image at column 0 (dropping the padding + mid-row placement)
    /// collapses all three anchors onto `(0, row)` and fails the anchor assertion while leaving the
    /// pixel and text assertions passing.
    #[test]
    fn encoded_history_replays_into_identical_images() {
        let mut em = Emulator::new(20, 4);
        // Row 0: an image at column 0, then text.
        em.advance(&kitty(1, 2, 2));
        em.advance(b"alpha\r\n");
        // Row 1: text, an image mid-row, more text.
        em.advance(b"be");
        em.advance(&kitty(2, 1, 3));
        em.advance(b"ta\r\n");
        // Row 2: short text, then an image far past its last glyph.
        em.advance(b"hi\x1b[2;10H");
        em.advance(b"\x1b[3;10H");
        em.advance(&kitty(3, 3, 1));

        let before: Vec<Image> = em.screen().images().to_vec();
        assert_eq!(before.len(), 3, "the fixture placed three images");
        assert_eq!(
            before.iter().map(|i| i.anchor).collect::<Vec<_>>(),
            vec![(0, 0), (2, 1), (9, 2)],
            "the fixture must place them at THREE different columns to discriminate",
        );

        let bytes = em.history_bytes(HistoryLimits {
            lines: 1000,
            image_bytes: 1 << 20,
        });
        let replayed = emulate(20, 4, &bytes);

        let after: Vec<Image> = replayed.screen().images().to_vec();
        assert_eq!(after.len(), 3, "every image came back");
        for (want, got) in before.iter().zip(&after) {
            assert_eq!(got.id, want.id, "image id");
            assert_eq!((got.width, got.height), (want.width, want.height), "extent");
            assert_eq!(got.rgba, want.rgba, "exact pixels");
            assert_eq!(got.anchor, want.anchor, "anchor CELL");
        }
        assert_eq!(
            retained(replayed.screen()),
            retained(em.screen()),
            "the text around the images is unchanged — the padding is content-neutral",
        );
    }

    /// Images are dropped when the BUDGET cannot hold them, and the text is unaffected — the bound that
    /// keeps one save tick from writing gigabytes.
    ///
    /// `image_bytes: 0` is the text-only encoding (the pre-image behaviour, and what
    /// `SPRAG_RESTORE_IMAGE_BYTES=0` selects). A budget that fits only the first image keeps that one
    /// and drops the rest, so the oldest survives — predictable, unlike dropping by size.
    ///
    /// REVERT-PROOF: ignoring the budget replays 2 images under `image_bytes: 0` and 2 under the
    /// one-image budget, failing both counts; the text assertion passes either way, which is why the
    /// counts are what this asserts.
    #[test]
    fn the_image_budget_bounds_what_a_history_carries() {
        let mut em = Emulator::new(20, 4);
        em.advance(b"text");
        em.advance(&kitty(1, 2, 2)); // 2*2*4 = 16 raster bytes
        em.advance(&kitty(2, 2, 2)); // another 16
        assert_eq!(em.screen().images().len(), 2);

        let text_only = emulate(20, 4, &em.history_bytes(HistoryLimits::text_only(1000)));
        assert!(
            text_only.screen().images().is_empty(),
            "a zero budget keeps the text and drops every image",
        );
        assert_eq!(
            retained(text_only.screen()),
            retained(em.screen()),
            "...and the text is untouched by the drop",
        );

        let one = emulate(
            20,
            4,
            &em.history_bytes(HistoryLimits {
                lines: 1000,
                image_bytes: 16,
            }),
        );
        assert_eq!(
            one.screen().images().len(),
            1,
            "a budget for one raster keeps exactly one",
        );
        assert_eq!(one.screen().images()[0].id, 1, "and it is the OLDEST");
    }

    /// An image on a row of BLANKS survives — the trailing-blank trim must not take it with the row.
    ///
    /// An image displayed below the last line of text sits on exactly such a row (a shell prints an
    /// image after its prompt line), so this is the ordinary case, not an edge one.
    ///
    /// The screen is a row taller than the content needs, deliberately: the encoding ends every logical
    /// line with `CR LF` (so a restored pane's first output starts fresh), and on a screen the content
    /// exactly FILLS, that final newline scrolls — carrying the image up with the text it sits beside.
    /// The image stays with its content either way, but the absolute row differs, and this test is
    /// about the TRIM, so it does not put the two properties in one assertion. See the trailing-newline
    /// bound in this module's docs.
    ///
    /// REVERT-PROOF: trimming by cells alone (ignoring the anchors) drops the row and the image with
    /// it, so the replay carries none.
    #[test]
    fn an_image_on_an_otherwise_blank_row_is_not_trimmed_away() {
        let mut em = Emulator::new(20, 5);
        em.advance(b"prompt\r\n");
        em.advance(b"\x1b[4;3H"); // row 3 (0-based), col 2 — nothing written there
        em.advance(&kitty(9, 2, 2));
        assert_eq!(em.screen().images()[0].anchor, (2, 3));

        let replayed = emulate(
            20,
            5,
            &em.history_bytes(HistoryLimits {
                lines: 1000,
                image_bytes: 1 << 20,
            }),
        );
        assert_eq!(
            replayed.screen().images().len(),
            1,
            "the image outlived a row that held no text",
        );
        assert_eq!(
            replayed.screen().images()[0].anchor,
            (2, 3),
            "and came back at its own cell — two blank rows above it were emitted, not skipped",
        );
    }

    /// The trailing newline's cost, stated as a test rather than left as a surprise: when the content
    /// exactly fills the screen, the encoding's final `CR LF` scrolls it, so an image comes back one row
    /// HIGHER — still beside the same text, since that scrolled too.
    ///
    /// Pinning it is the point. This is a property of the deliberate "history ends at a line boundary"
    /// choice (which is what keeps a restored shell's prompt off the last recorded line), not of the
    /// image path, and it applies to the text identically — it is only VISIBLE on an image because an
    /// anchor is an absolute cell while text is compared as a content stream.
    #[test]
    fn a_screen_filling_history_scrolls_its_image_up_with_its_text() {
        let mut em = Emulator::new(20, 2);
        em.advance(b"first\r\nsecond");
        em.advance(&kitty(4, 1, 1));
        assert_eq!(em.screen().images()[0].anchor, (6, 1), "beside `second`");

        let replayed = emulate(
            20,
            2,
            &em.history_bytes(HistoryLimits {
                lines: 1000,
                image_bytes: 1 << 20,
            }),
        );
        let img = replayed.screen().images();
        assert_eq!(img.len(), 1, "the image survived the scroll");
        assert_eq!(
            img[0].anchor,
            (6, 0),
            "one row up — and `second` moved up with it",
        );
        assert!(
            replayed.screen().row_text(0).starts_with("second"),
            "the image is still beside the text it was captured with: {:?}",
            replayed.screen().row_text(0),
        );
    }

    /// A Kitty `a=T` RGBA transmit for a `w x h` image with id `id`, pixels derived from the id so two
    /// fixtures never share a raster.
    fn kitty(id: u32, w: u32, h: u32) -> Vec<u8> {
        use base64::Engine as _;
        let px: Vec<u8> = (0..(w * h * 4)).map(|i| (i + id * 7) as u8).collect();
        let b64 = base64::engine::general_purpose::STANDARD.encode(&px);
        format!("\x1b_Ga=T,f=32,s={w},v={h},i={id};{b64}\x1b\\").into_bytes()
    }

    /// A soft-wrapped line comes back SOFT-wrapped, not as two hard lines.
    ///
    /// This axis is invisible to every other assertion here, which is why it gets its own test.
    /// Breaking a line at its wrap point lays out exactly the same characters in exactly the same
    /// columns, so cell content is identical; and it is CONSISTENTLY lossy, so re-encoding the
    /// replay reproduces the same (already-flattened) bytes — a fixed-point check cannot see it
    /// either. Only the wrap flags differ, and they are what lets a later reflow rejoin the line at
    /// a new width. So the flags are what this compares.
    ///
    /// The fixture keeps the wrapped line in the VISIBLE grid because that is where the flags have
    /// a public reader.
    #[test]
    fn a_soft_wrapped_line_replays_soft_wrapped() {
        let flags = |screen: &Screen| {
            (0..screen.rows())
                .map(|r| screen.wrapped(r))
                .collect::<Vec<_>>()
        };
        let em = emulate(
            20,
            6,
            b"this line is long enough to soft wrap past twenty columns\r\n",
        );
        // Non-vacuous: the fixture really does wrap, across two continuation rows.
        assert_eq!(
            flags(em.screen()),
            vec![true, true, false, false, false, false],
            "the fixture must soft-wrap over three rows",
        );
        let replayed = emulate(20, 6, &em.history_bytes(HistoryLimits::text_only(1000)));
        assert_eq!(
            flags(replayed.screen()),
            flags(em.screen()),
            "the soft wrap came back as a hard break, so a later reflow can no longer rejoin it",
        );
    }

    /// Shell-integration marks ride the encoding, so a restored pane keeps its jump-to-prompt
    /// targets and its last command's exit status rather than coming back semantically blank.
    #[test]
    fn prompt_marks_and_exit_status_survive_the_round_trip() {
        // One mark per ROW is the data model, so the fixture gives each boundary its own row.
        let original = emulate(
            20,
            6,
            b"\x1b]133;A\x1b\\$ ls\r\n\x1b]133;C\x1b\\out\r\n\x1b]133;D;3\x1b\\\r\n\x1b]133;A\x1b\\$ ",
        );
        let replayed = emulate(
            20,
            6,
            &original.history_bytes(HistoryLimits::text_only(1000)),
        );
        // Non-vacuous: the fixture really did record both a prompt target and an exit status.
        assert_eq!(original.screen().prompt_positions(), vec![0, 3]);
        assert_eq!(
            original.screen().last_command().and_then(|c| c.exit_status),
            Some(3),
        );
        assert_eq!(
            replayed.screen().prompt_positions(),
            original.screen().prompt_positions(),
            "the jump-to-prompt targets moved",
        );
        assert_eq!(
            replayed.screen().last_command().and_then(|c| c.exit_status),
            Some(3),
            "the recorded command's exit status did not survive",
        );
    }

    /// The bound is the NEWEST logical lines, so a limit smaller than the history keeps the tail —
    /// the part a user is looking at — and drops the oldest.
    #[test]
    fn the_limit_keeps_the_newest_logical_lines() {
        let em = emulate(20, 4, b"one\r\ntwo\r\nthree\r\nfour\r\n");
        let replayed = emulate(20, 4, &em.history_bytes(HistoryLimits::text_only(2)));
        let text = replayed.screen().full_text();
        assert_eq!(text, "three\nfour", "the newest two lines, oldest dropped");
    }

    /// `limit == 0` is the "history persistence is off" answer: nothing at all, not an empty line.
    #[test]
    fn a_zero_limit_encodes_nothing() {
        let em = emulate(20, 4, b"one\r\ntwo\r\n");
        assert!(em.history_bytes(HistoryLimits::text_only(0)).is_empty());
    }

    /// The blank grid below a short screen is padding, not history: a two-line pane encodes two
    /// lines, so a restored pane's new prompt does not open under a screenful of blanks.
    #[test]
    fn trailing_blank_rows_are_not_encoded() {
        let em = emulate(20, 24, b"one\r\ntwo\r\n");
        let bytes = em.history_bytes(HistoryLimits::text_only(1000));
        assert_eq!(
            bytes.iter().filter(|b| **b == b'\n').count(),
            2,
            "only the two content lines are encoded: {:?}",
            String::from_utf8_lossy(&bytes),
        );
    }

    /// A soft-wrap CONTINUATION row is emitted at its full width, blanks included, because its
    /// trailing blanks are not padding — they are the middle of a logical line. Only the row that
    /// ENDS the line may be trimmed.
    ///
    /// The discriminating case is a line whose wrap point falls inside a run of spaces: those
    /// cells are indistinguishable from right-margin padding, yet dropping them pulls everything
    /// after them left.
    #[test]
    fn a_wrapped_row_keeps_its_trailing_spaces() {
        // Three characters plus seventeen spaces exactly fill a twenty-column row, so the row is a
        // continuation whose last seventeen cells compare equal to blank.
        let line = format!("abc{}def", " ".repeat(17));
        let em = emulate(20, 4, format!("{line}\r\n").as_bytes());
        assert_eq!(
            em.screen().find("def").matches.first().map(|m| m.col),
            Some(0),
            "the fixture must wrap so that `def` starts the continuation row",
        );
        let replayed = emulate(20, 4, &em.history_bytes(HistoryLimits::text_only(1000)));
        assert_eq!(
            retained(replayed.screen()),
            retained(em.screen()),
            "the wrapped line's interior spaces were dropped, shifting the text after them",
        );
    }

    /// A line shorter than the terminal is stored at its own length, not padded to the right
    /// margin. The grid pads every row to `cols`; carrying that padding into the durable form
    /// would inflate a mostly-empty pane's history by the width of its terminal per line.
    #[test]
    fn a_short_line_does_not_encode_its_right_margin_padding() {
        let em = emulate(20, 4, b"hi\r\n");
        let text =
            String::from_utf8_lossy(&em.history_bytes(HistoryLimits::text_only(1000))).into_owned();
        assert!(
            !text.contains("hi "),
            "the line was padded to the margin: {text:?}",
        );
    }

    /// The encoding leaves the terminal at a default pen with no open hyperlink, even when the
    /// history itself ends mid-style. Without it a restored pane's OWN first output — a fresh
    /// shell's prompt — would inherit whatever attributes and link the last recorded cell carried.
    #[test]
    fn the_encoding_closes_the_pen_and_any_open_hyperlink() {
        let em = emulate(
            20,
            4,
            b"\x1b]8;;https://example.com\x1b\\\x1b[1;31mbold linked\r\n",
        );
        let mut replayed = emulate(20, 4, &em.history_bytes(HistoryLimits::text_only(1000)));
        replayed.advance(b"X"); // the restored pane's first fresh output
        let fresh = replayed.screen().cell(0, 1).expect("the X cell");
        assert_eq!(fresh.cluster, "X", "the fresh output landed elsewhere");
        assert_eq!(
            fresh.attrs,
            Attrs::default(),
            "history's pen leaked into the restored pane's own output",
        );
        assert_eq!(fresh.fg, Color::Default, "history's colour leaked");
        assert!(
            fresh.hyperlink.is_none(),
            "history left a hyperlink open over the restored pane's output",
        );
    }

    /// While a fullscreen app holds the ALTERNATE screen, history is the MAIN screen's — the
    /// alternate buffer is transient app furniture and carries no scrollback at all, so encoding
    /// the active screen would persist a redrawable UI and lose the real history.
    #[test]
    fn the_alternate_screen_encodes_the_main_screens_history() {
        let mut em = emulate(20, 4, b"real history\r\n");
        em.advance(b"\x1b[?1049h"); // enter the alt screen
        em.advance(b"transient app UI");
        let text =
            String::from_utf8_lossy(&em.history_bytes(HistoryLimits::text_only(1000))).into_owned();
        assert!(
            text.contains("real history"),
            "the main screen's history was lost: {text:?}",
        );
        assert!(
            !text.contains("transient app UI"),
            "the alt screen's furniture was persisted: {text:?}",
        );
    }
}
