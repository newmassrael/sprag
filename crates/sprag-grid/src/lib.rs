//! sprag-grid — project a [`sprag_vt::Screen`] into a pinion `GridBuffer`.
//!
//! DESIGN.md §3: sprag (the producer) owns the authoritative terminal
//! state; pinion holds a retained projection. This crate is the
//! one-directional adapter — a fresh `GridBuffer` per frame, mapping the
//! port's cells/cursor/screen-kind/damage onto pinion's data model.
//! Because both sides model the same axes, this is a flat mapping rather
//! than a translation.

use std::collections::HashMap;
use std::sync::Arc;

use pinion_core::style::Color as PinColor;
use pinion_core::{
    CellAttrs, CursorShape as PinCursorShape, GridBuffer, GridCursor, Hyperlink as PinHyperlink,
    HyperlinkId, ScreenKind as PinScreenKind, TermCell, TermColor,
    UnderlineStyle as PinUnderlineStyle,
};
use sprag_vt::{
    Attrs, Cell, Color, ColorTarget, Cursor, CursorShape, Hyperlink, Palette, Screen, ScreenKind,
    UnderlineStyle, Width,
};

/// Project a screen into a fresh pinion `GridBuffer`, resolving each cell's colour
/// against the terminal's live [`Palette`] (the OSC-colour SSOT).
///
/// Cells store their colour symbolically ([`Color::Indexed`] / [`Color::Default`]),
/// so resolving here — every frame, against the live palette — is what makes an
/// `OSC 4 / 10 / 11` change re-colour the whole screen. An un-mutated palette
/// resolves to the standard xterm values, byte-identical to the pre-OSC-colour
/// projection. pinion replaces the node's buffer wholesale each frame (no per-cell
/// mutation), so a new buffer per call is the intended shape.
#[must_use]
pub fn project(screen: &Screen, palette: &Palette) -> GridBuffer {
    let cols = screen.cols();
    let rows = screen.rows();
    let mut buffer = GridBuffer::new(cols, rows);
    let mut interner = HyperlinkInterner::default();

    for row in 0..rows {
        buffer = buffer.with_row(row, project_row(screen, row, cols, &mut interner, palette));
        if let Some(generation) = screen.row_generation(row) {
            buffer = buffer.with_row_generation(row, generation);
        }
    }

    buffer = buffer.with_cursor(project_cursor(screen.cursor(), palette));
    buffer
        .with_screen(screen_kind(screen.screen_kind()))
        .with_hyperlinks(interner.table)
}

/// Project a screen into a `GridBuffer` scrolled up by `offset_lines` rows of
/// history. `offset_lines == 0` is the live view, byte-identical to [`project`].
///
/// A positive offset shows the pane's scrollback: the displayed window of
/// `screen.rows()` logical rows ends `offset_lines` rows above the live bottom.
/// Rows in the scrollback region are projected from their **stored styled cells**
/// ([`Screen::scrollback_cells`]) via `project_glyph_row`, so scrolled history
/// keeps its original fg/bg/attrs (and wide clusters their head/trailer split) —
/// identical styling to the live region. Rows still in the visible region keep
/// their exact cells. The cursor is omitted while scrolled (it lives in the live
/// region below the view). `offset_lines` is clamped to the retained scrollback
/// depth.
#[must_use]
pub fn project_scrolled(screen: &Screen, offset_lines: usize, palette: &Palette) -> GridBuffer {
    if offset_lines == 0 {
        return project(screen, palette);
    }
    let cols = screen.cols();
    let rows = screen.rows();
    let scrollback: Vec<&[Cell]> = screen.scrollback_cells().collect();
    let scrollback_len = scrollback.len();
    let offset = offset_lines.min(scrollback_len);
    if offset == 0 {
        // A stale positive offset against now-empty scrollback (the screen
        // cleared its history, or switched to the alternate screen) IS the live
        // view — return it with the cursor, not a cursor-less window.
        return project(screen, palette);
    }
    // First displayed row's index into the logical [scrollback .. visible]
    // sequence: the window of `rows` rows ends `offset` above the live bottom
    // (offset <= scrollback_len, so this never underflows).
    let top = scrollback_len - offset;

    let mut buffer = GridBuffer::new(cols, rows);
    let mut interner = HyperlinkInterner::default();
    for display in 0..rows {
        let logical = top + display as usize;
        let cells = if logical < scrollback_len {
            project_glyph_row(scrollback[logical], cols, &mut interner, palette)
        } else {
            project_row(
                screen,
                (logical - scrollback_len) as u16,
                cols,
                &mut interner,
                palette,
            )
        };
        buffer = buffer.with_row(display, cells);
    }
    // No cursor while scrolled; the screen kind matches the live screen.
    buffer
        .with_screen(screen_kind(screen.screen_kind()))
        .with_hyperlinks(interner.table)
}

/// Overlay an in-progress IME preedit (composition) string onto `buffer` at its
/// cursor, drawn underlined to mark it as composing.
///
/// **This is the one source of the rationale** the host/GUI seams cross-reference:
/// under winit + XIM the platform IME does **not** paint the in-progress preedit
/// over-the-spot; it emits an `Ime::Preedit` for the application to render. So a
/// terminal must draw the half-composed syllable itself — this overlay is that
/// rendering, the visual feedback that makes Hangul/CJK composition visible
/// before commit. It is display-only: the preedit never reaches the PTY — only a
/// committed [`CompositionEvent::Commit`](pinion_core::CompositionEvent) writes
/// (via the host text seam), at which point the committed glyphs arrive through
/// the PTY and the overlay clears.
///
/// Width comes from the producer's authority ([`sprag_vt::char_columns`]) — the
/// same model the emulator prints with — not a second width computation, so a
/// composing wide (CJK) syllable occupies the two cells (head + trailer) its
/// committed form will. A wide head whose trailer would fall off the row's right
/// edge is clipped *whole* (never emitted as a malformed Narrow-tagged wide
/// cell), a narrow cluster past the edge is clipped, and zero-width combining
/// marks are skipped (the emulator's documented grapheme-cluster gap).
///
/// No-ops (returns `buffer` unchanged) when the preedit is empty (no active
/// composition) **or** the buffer has no visible cursor — the cursor is the
/// compose anchor, so a scrolled-back history window (cursor-less per
/// [`project_scrolled`]) or a DECTCEM-hidden cursor has nowhere to anchor. This
/// is the single "no cursor ⇒ no overlay" gate; callers need no offset check.
#[must_use]
pub fn overlay_preedit(buffer: GridBuffer, preedit: &str) -> GridBuffer {
    let cursor = buffer.cursor();
    if preedit.is_empty() || !cursor.visible {
        return buffer;
    }
    let cols = buffer.cols();
    let row = cursor.row;
    if cols == 0 || row >= buffer.rows() {
        return buffer;
    }
    // Copy the cursor row, then splice the preedit in starting at the cursor
    // column. `with_row` rewrites the whole row, so the copy preserves the
    // committed cells to the left/right of the composition. Every (col, row)
    // here is in bounds (col < cols, row < rows checked above), so `cell` is
    // always `Some` — a `None` would be an internal `GridBuffer` invariant break.
    let cols_usize = usize::from(cols);
    let mut cells: Vec<TermCell> = (0..cols)
        .map(|col| {
            buffer
                .cell(col, row)
                .cloned()
                .expect("col < cols and row < rows hold above")
        })
        .collect();
    let mut col = usize::from(cursor.col);
    for ch in preedit.chars() {
        match sprag_vt::char_columns(ch) {
            // Combining mark: merged into the previous cell by the emulator; the
            // overlay has no previous-cell to merge into, so skip (the gap).
            0 => continue,
            // Wide (CJK) head needs its column AND the trailer column; clip the
            // whole head if the trailer would run off the edge (no lone wide head).
            2 => {
                if col + 1 >= cols_usize {
                    break;
                }
                let wide = preedit_cell(ch).wide();
                let trailer = wide.trailer();
                cells[col] = wide;
                cells[col + 1] = trailer;
                col += 2;
            }
            // Narrow cluster (1 column; any unexpected >2 width also renders narrow).
            _ => {
                if col >= cols_usize {
                    break;
                }
                cells[col] = preedit_cell(ch);
                col += 1;
            }
        }
    }
    buffer.with_row(row, cells)
}

/// Overlay a mouse text selection onto `buffer` by INVERTING (toggling `reverse`)
/// each selected cell — the xterm-idiomatic selection highlight (a cell already in
/// reverse video reads normal under the band, exactly as a real terminal inverts the
/// selected region). `start` and `end` are `(col, row)` in the buffer's VISIBLE grid,
/// in reading (row-major) order (`start <= end`), both INCLUSIVE — a linear / stream
/// selection: the first row runs from `start.col` to the row end, whole middle rows,
/// the last row up to `end.col`. Rows / columns past the buffer are clamped; the caller
/// gates out an empty span (a zero-width click selects nothing), so this always inverts
/// at least the span it is given.
///
/// Like [`overlay_preedit`] this rewrites only the affected rows wholesale
/// ([`GridBuffer::with_row`]) — the display-only projection is rebuilt every frame, so
/// mutating the projected buffer never touches the producer's authoritative cells.
#[must_use]
pub fn overlay_selection(buffer: GridBuffer, start: (u16, u16), end: (u16, u16)) -> GridBuffer {
    let cols = buffer.cols();
    let rows = buffer.rows();
    if cols == 0 || rows == 0 {
        return buffer;
    }
    let (start_col, start_row) = start;
    let (end_col, end_row) = end;
    let last_row = end_row.min(rows - 1);
    let mut buffer = buffer;
    let mut row = start_row;
    while row <= last_row {
        let first = if row == start_row { start_col } else { 0 };
        let last_incl = if row == end_row { end_col } else { cols - 1 }.min(cols - 1);
        if first <= last_incl {
            let cells: Vec<TermCell> = (0..cols)
                .map(|col| {
                    let cell = buffer
                        .cell(col, row)
                        .cloned()
                        .unwrap_or_else(TermCell::blank);
                    if col >= first && col <= last_incl {
                        let attrs = cell.attrs;
                        cell.with_attrs(attrs.with_reverse(!attrs.reverse))
                    } else {
                        cell
                    }
                })
                .collect();
            buffer = buffer.with_row(row, cells);
        }
        row += 1;
    }
    buffer
}

/// One search-match span to highlight, in the buffer's VISIBLE grid: `(row, col, cols)`.
/// Rows / columns past the buffer are clamped by [`overlay_matches`]; a zero-`cols` span paints
/// nothing.
pub type MatchSpan = (u16, u16, u16);

/// Overlay find-in-scrollback match highlights onto `buffer` by RECOLOURING each matched cell to
/// `fg` / `bg` — the third overlay of the family, and the one that must NOT be reverse-video.
///
/// [`overlay_selection`] and [`overlay_hyperlink_hover`] both invert, which is right for a state the
/// user is holding (a drag, a hover) but wrong here: a match can coincide with a selection, and two
/// inversions would cancel to "no highlight at all" over exactly the cells the user is looking at. A
/// distinct colour pair also lets a caller mark the CURRENT match differently from the rest — the
/// same decision tmux encodes as `copy-mode-match-style` vs `copy-mode-current-match-style`.
///
/// The colours are the CALLER's (a display concern), passed as [`TermColor`]s so an
/// [`Indexed`](TermColor::Indexed) pair resolves through the pane's own live palette and stays
/// theme-consistent. Like its siblings this rewrites only the affected rows wholesale on the
/// per-frame projected buffer, never the producer's authoritative cells.
#[must_use]
pub fn overlay_matches(
    buffer: GridBuffer,
    spans: &[MatchSpan],
    fg: TermColor,
    bg: TermColor,
) -> GridBuffer {
    let cols = buffer.cols();
    let rows = buffer.rows();
    if cols == 0 || rows == 0 || spans.is_empty() {
        return buffer;
    }
    let mut buffer = buffer;
    for row in 0..rows {
        // Rewrite a row only if a span actually lands on it — most rows carry no match.
        let on_row: Vec<MatchSpan> = spans
            .iter()
            .copied()
            .filter(|&(span_row, _, span_cols)| span_row == row && span_cols > 0)
            .collect();
        if on_row.is_empty() {
            continue;
        }
        let cells: Vec<TermCell> = (0..cols)
            .map(|col| {
                let cell = buffer
                    .cell(col, row)
                    .cloned()
                    .unwrap_or_else(TermCell::blank);
                let hit = on_row
                    .iter()
                    .any(|&(_, start, width)| col >= start && col < start.saturating_add(width));
                // Recolour IN PLACE rather than rebuilding: only the two colour axes change, so the
                // cell keeps its attrs, width role, hyperlink and underline colour — a match on a
                // bold link stays a bold link.
                let mut cell = cell;
                if hit {
                    cell.fg = fg;
                    cell.bg = bg;
                }
                cell
            })
            .collect();
        buffer = buffer.with_row(row, cells);
    }
    buffer
}

/// Overlay the OSC-8 hyperlink hover highlight (R-71.2, pinion R1405): reverse-video
/// every cell whose link matches `hovered` — the WHOLE id-group at once, so a link
/// split across a wrap lights together (its non-adjacent runs share one
/// [`HyperlinkId`] in this buffer's interning table). `None` (nothing hovered) is a
/// no-op returning `buffer` unchanged.
///
/// `hovered` is a [`HyperlinkId`] INTO THIS buffer's table: the caller resolves the
/// pointer's cell to its link id (the cell's own [`TermCell::hyperlink`], obtained
/// e.g. via [`GridBuffer::cell`]) and passes it. Like [`overlay_selection`] this
/// rewrites only affected rows wholesale on the per-frame projected buffer, never the
/// producer's authoritative cells. The URI a click opens is resolved separately via
/// [`GridBuffer::cell_hyperlink`].
#[must_use]
pub fn overlay_hyperlink_hover(buffer: GridBuffer, hovered: Option<HyperlinkId>) -> GridBuffer {
    let Some(hovered) = hovered else {
        return buffer;
    };
    let cols = buffer.cols();
    let rows = buffer.rows();
    let mut buffer = buffer;
    for row in 0..rows {
        // Rewrite a row only if it actually carries a cell of the hovered link.
        let touched = (0..cols).any(|col| {
            buffer
                .cell(col, row)
                .is_some_and(|c| c.hyperlink == Some(hovered))
        });
        if !touched {
            continue;
        }
        let cells: Vec<TermCell> = (0..cols)
            .map(|col| {
                let cell = buffer
                    .cell(col, row)
                    .cloned()
                    .unwrap_or_else(TermCell::blank);
                if cell.hyperlink == Some(hovered) {
                    let attrs = cell.attrs;
                    cell.with_attrs(attrs.with_reverse(!attrs.reverse))
                } else {
                    cell
                }
            })
            .collect();
        buffer = buffer.with_row(row, cells);
    }
    buffer
}

/// One preedit cell: the char in default colors, underlined to mark an
/// in-progress composition. The grid cursor (a block at the compose position)
/// highlights the active cell; the underline distinguishes the rest of the
/// composing run from committed text.
fn preedit_cell(ch: char) -> TermCell {
    TermCell::new(ch.to_string(), TermColor::Default, TermColor::Default)
        .with_attrs(CellAttrs::empty().with_underline(true))
}

/// Build a scrolled-back (history) row's `TermCell`s from its STORED cells,
/// preserving fg/bg/attrs — so scrollback paints in its original colors, not flat
/// plain text. Wide heads expand into pinion's head + trailer pair (the same shape
/// [`project_row`] gives the live grid). Both scrollback push paths store a row as
/// head+trailer pairs (`scroll_region_up` copies the live grid row; `reflowed` Pass 2
/// regenerates the trailer via `Cell::trailer_for`), so a stored `Width::Trailer`
/// is REDUNDANT — its head already emitted the pair — and is skipped here. The one
/// trailerless input is the degenerate `cols == 1` lone wide head (the emulator
/// stores a head with no room for a trailer); it lands in the `Width::Wide`
/// no-room arm below and renders narrow (matching `project_row`'s edge clip).
/// Padded with blanks / truncated to `cols`.
fn project_glyph_row(
    glyphs: &[Cell],
    cols: u16,
    interner: &mut HyperlinkInterner,
    palette: &Palette,
) -> Vec<TermCell> {
    let ncols = cols as usize;
    let mut out = Vec::with_capacity(ncols);
    for cell in glyphs {
        let col = out.len();
        if col >= ncols {
            break;
        }
        match cell.width {
            Width::Wide if col + 1 < ncols => push_wide_pair(&mut out, cell, interner, palette),
            Width::Trailer => {}
            _ => out.push(term_cell(cell, interner, palette)),
        }
    }
    while out.len() < ncols {
        out.push(TermCell::blank());
    }
    out
}

/// Build one row's `TermCell`s, expanding wide heads into pinion's
/// head + trailer pair (DESIGN.md §3: producer determines width).
fn project_row(
    screen: &Screen,
    row: u16,
    cols: u16,
    interner: &mut HyperlinkInterner,
    palette: &Palette,
) -> Vec<TermCell> {
    let mut out = Vec::with_capacity(cols as usize);
    let mut col = 0;
    while col < cols {
        let Some(cell) = screen.cell(col, row) else {
            break;
        };
        match cell.width {
            Width::Wide if col + 1 < cols => {
                push_wide_pair(&mut out, cell, interner, palette);
                col += 2;
            }
            // An orphan trailer means the head was clipped at the edge;
            // emit a blank so the column count stays exact.
            Width::Trailer => {
                out.push(TermCell::blank());
                col += 1;
            }
            _ => {
                out.push(term_cell(cell, interner, palette));
                col += 1;
            }
        }
    }
    out
}

/// Assigns each distinct OSC-8 hyperlink a pinion [`HyperlinkId`] for ONE
/// projected [`GridBuffer`], deduping by the sprag-vt `Arc<Hyperlink>` POINTER.
///
/// The emulator gives every cell of one link — including its wrap
/// continuations — the same `Arc` (its OSC-8 pen), so pointer identity
/// collapses them to a single table entry and thus one id, while two anonymous
/// links to the same URI (distinct `Arc`s) stay distinct entries. Value
/// equality would wrongly merge those, so the dedup is deliberately by pointer
/// — matching the emulator's own grouping (a `Some(id)` link is already one
/// `Arc` across its runs; an anonymous link is one `Arc` per run). The
/// assembled [`table`](Self::table) is handed to [`GridBuffer::with_hyperlinks`]
/// wholesale, exactly like the cells (producer-owned state).
#[derive(Default)]
struct HyperlinkInterner {
    table: Vec<PinHyperlink>,
    by_ptr: HashMap<*const Hyperlink, HyperlinkId>,
}

impl HyperlinkInterner {
    /// Resolve a cell's optional link to its [`HyperlinkId`], adding a fresh
    /// table entry the first time a given `Arc` is seen this projection.
    fn intern(&mut self, link: Option<&Arc<Hyperlink>>) -> Option<HyperlinkId> {
        let arc = link?;
        let ptr = Arc::as_ptr(arc);
        if let Some(&id) = self.by_ptr.get(&ptr) {
            return Some(id);
        }
        let id = HyperlinkId(u32::try_from(self.table.len()).unwrap_or(u32::MAX));
        let mut entry = PinHyperlink::new(arc.uri.clone());
        if let Some(group) = &arc.id {
            entry = entry.with_id(group.clone());
        }
        self.table.push(entry);
        self.by_ptr.insert(ptr, id);
        Some(id)
    }
}

fn term_cell(cell: &Cell, interner: &mut HyperlinkInterner, palette: &Palette) -> TermCell {
    let mut tc = TermCell::new(
        cell.cluster.to_string(),
        term_color(cell.fg, ColorTarget::Foreground, palette),
        term_color(cell.bg, ColorTarget::Background, palette),
    )
    .with_attrs(cell_attrs(cell.attrs));
    // SGR 58 underline colour (orthogonal to the style axis). `None` is the
    // SGR-59 default — pinion then draws the underline in the cell's own fg.
    if let Some(color) = cell.underline_color {
        tc = tc.with_underline_color(term_color(color, ColorTarget::Foreground, palette));
    }
    // OSC-8 hyperlink: intern the link into this buffer's table and stamp the
    // cell with its id (pinion resolves the id -> uri at paint / snapshot time).
    if let Some(id) = interner.intern(cell.hyperlink.as_ref()) {
        tc = tc.with_hyperlink(id);
    }
    tc
}

/// Push a wide cluster as pinion's head + trailer pair (DESIGN.md §3: producer
/// determines width). Shared by the live-grid [`project_row`] and the history
/// [`project_glyph_row`], which differ in iteration but emit wide cells the same way.
/// pinion's [`TermCell::trailer`] copies the head's hyperlink, so the wide glyph's
/// continuation column stays part of the same link.
fn push_wide_pair(
    out: &mut Vec<TermCell>,
    cell: &Cell,
    interner: &mut HyperlinkInterner,
    palette: &Palette,
) {
    let head = term_cell(cell, interner, palette).wide();
    let trailer = head.trailer();
    out.push(head);
    out.push(trailer);
}

/// Resolve a sprag-vt cell [`Color`] to a pinion [`TermColor`] against the live
/// [`Palette`]. Every colour resolves to a concrete [`TermColor::Rgb`]: `Indexed`
/// and `Default` are looked up in the palette (so an OSC-colour override is honoured
/// and the whole screen re-colours on a palette change), `Rgb` passes through. An
/// un-mutated palette yields the standard xterm value pinion would itself resolve to,
/// so this is transparent until the child changes a colour.
fn term_color(color: Color, target: ColorTarget, palette: &Palette) -> TermColor {
    let rgb = palette.resolve(color, target);
    TermColor::Rgb(PinColor::rgb(rgb.r, rgb.g, rgb.b))
}

fn cell_attrs(attrs: Attrs) -> CellAttrs {
    CellAttrs::empty()
        .with_bold(attrs.bold)
        .with_dim(attrs.dim)
        .with_italic(attrs.italic)
        .with_underline_style(pin_underline(attrs.underline))
        .with_blink(attrs.blink)
        .with_reverse(attrs.reverse)
        .with_hidden(attrs.hidden)
        .with_strikethrough(attrs.strikethrough)
}

/// Map the port's [`UnderlineStyle`] to pinion's — the two enums share
/// their six SGR 4:x variants one-for-one.
fn pin_underline(style: UnderlineStyle) -> PinUnderlineStyle {
    match style {
        UnderlineStyle::None => PinUnderlineStyle::None,
        UnderlineStyle::Single => PinUnderlineStyle::Single,
        UnderlineStyle::Double => PinUnderlineStyle::Double,
        UnderlineStyle::Curly => PinUnderlineStyle::Curly,
        UnderlineStyle::Dotted => PinUnderlineStyle::Dotted,
        UnderlineStyle::Dashed => PinUnderlineStyle::Dashed,
    }
}

fn cursor_shape(shape: CursorShape) -> PinCursorShape {
    match shape {
        CursorShape::Block => PinCursorShape::Block,
        CursorShape::Bar => PinCursorShape::Bar,
        CursorShape::Underline => PinCursorShape::Underline,
    }
}

/// Project the terminal's cursor into pinion's [`GridCursor`] — all four axes it carries: position,
/// shape, DECTCEM visibility, the DECSCUSR blink MODE (pinion R1425) and the `OSC 12` colour
/// (pinion R1424). The one place a cursor crosses into pinion, so [`project`] and
/// [`project_scrolled`]'s live path cannot describe it two ways.
///
/// The blink PHASE is not here and must not be: pinion owns it, running a per-window clock and
/// gating the overlay through `GridCursor::shown_this_phase`. sprag reports the mode; pinion
/// animates it. Likewise the hollow unfocused cursor (pinion R1427) is derived from OS window
/// focus at paint time, so it needs nothing from the projection.
///
/// The colour is chained only when `OSC 12` actually set one: pinion reads an absent
/// `cursor_color` as "draw the cursor from the cell it sits on", which is the reverse-video
/// default a terminal starts with. Passing the palette's cursor colour unconditionally would
/// paint every cursor that one absolute colour and lose that default over coloured text — which
/// is why the palette models the colour as an `Option` rather than a seeded RGB.
fn project_cursor(cursor: Cursor, palette: &Palette) -> GridCursor {
    let projected = GridCursor::new(
        cursor.col,
        cursor.row,
        cursor_shape(cursor.shape),
        cursor.visible,
    )
    .with_blink(cursor.blink);
    match palette.cursor_color() {
        // A direct `Color`, not a `TermColor`: `OSC 12` names an absolute colour, so — unlike a
        // cell's fg/bg — there is no palette index or theme default left to resolve later.
        Some(rgb) => projected.with_cursor_color(PinColor::rgb(rgb.r, rgb.g, rgb.b)),
        None => projected,
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

    /// The default (un-mutated) xterm palette these projection tests resolve against.
    fn palette() -> Palette {
        Palette::xterm_default()
    }

    #[test]
    fn projects_dimensions_and_text() {
        let screen = screen_from(b"hi", 10, 2);
        let buffer = project(&screen, &palette());
        assert_eq!(buffer.cols(), 10);
        assert_eq!(buffer.rows(), 2);
        assert_eq!(buffer.cell(0, 0).unwrap().cluster, "h");
        assert_eq!(buffer.cell(1, 0).unwrap().cluster, "i");
    }

    #[test]
    fn projects_indexed_color_and_bold() {
        let screen = screen_from(b"\x1b[1;31mA", 4, 1);
        let buffer = project(&screen, &palette());
        let cell = buffer.cell(0, 0).unwrap();
        // The projection resolves the palette: ANSI index 1 (red) -> xterm 0xcd0000.
        assert_eq!(cell.fg, TermColor::Rgb(PinColor::rgb(0xcd, 0x00, 0x00)));
        assert!(cell.attrs.bold);
    }

    /// A palette change RE-COLOURS already-projected cells: a cell printed in ANSI index 1 keeps its
    /// symbolic `Color::Indexed(1)`, so after an OSC 4 redefines index 1 the SAME cell re-projects to
    /// the new colour — the whole-screen re-colour a real terminal performs (data path; the live GUI
    /// repaints on the OSC's PTY-output `on_dirty`).
    #[test]
    fn a_palette_change_recolors_already_projected_cells() {
        use sprag_vt::Emulator;
        let mut em = Emulator::new(8, 1);
        em.advance(b"\x1b[31mX"); // "X" in ANSI index 1 (red)
        let before = project(VtPort::screen(&em), VtPort::palette(&em));
        assert_eq!(
            before.cell(0, 0).unwrap().fg,
            TermColor::Rgb(PinColor::rgb(0xcd, 0x00, 0x00)),
            "index 1 resolves to xterm red before the OSC 4"
        );
        // Redefine index 1 -> green; the same (still `Indexed(1)`) cell re-projects green.
        em.advance(b"\x1b]4;1;rgb:00/ff/00\x1b\\");
        let after = project(VtPort::screen(&em), VtPort::palette(&em));
        assert_eq!(
            after.cell(0, 0).unwrap().fg,
            TermColor::Rgb(PinColor::rgb(0x00, 0xff, 0x00)),
            "the already-printed cell re-colours to the redefined index 1"
        );
    }

    /// The projection resolves a `Color::Default` background against the palette's DYNAMIC
    /// background, so an OSC 11 change (which mutates that dynamic bg) re-colours the whole screen —
    /// the render half of OSC 11. A plain cell projects to the palette's `default_bg`, never a bare
    /// `TermColor::Default`. Revert-proof: with an un-mutated palette it resolves to xterm black.
    #[test]
    fn project_resolves_default_bg_against_the_palette_dynamic_background() {
        let screen = screen_from(b"x", 4, 1);
        // A palette whose default background was set (as OSC 11 would) to a distinct colour.
        let mut pal = Palette::xterm_default();
        pal.set_default_bg(sprag_vt::Rgb::new(0x10, 0x20, 0x30));
        let buffer = project(&screen, &pal);
        assert_eq!(
            buffer.cell(0, 0).unwrap().bg,
            TermColor::Rgb(PinColor::rgb(0x10, 0x20, 0x30)),
            "a default-bg cell resolves to the palette's dynamic background"
        );
        // The un-mutated palette resolves the same default bg to xterm black (transparent default).
        let plain = project(&screen, &palette());
        assert_eq!(
            plain.cell(0, 0).unwrap().bg,
            TermColor::Rgb(PinColor::rgb(0x00, 0x00, 0x00))
        );
    }

    /// A colored row scrolled off the top keeps its fg/attrs in the scrollback
    /// projection — the cell-based scrollback (was flattened to plain text, so
    /// history rendered colorless while the live prompt stayed colored).
    #[test]
    fn scrolled_off_row_keeps_color_and_attrs() {
        // 1-row screen: bold-red "A", then a newline scrolls it into scrollback.
        let screen = screen_from(b"\x1b[1;31mA\r\n", 4, 1);
        assert_eq!(screen.scrollback_len(), 1, "the A row scrolled off");
        let buf = project_scrolled(&screen, 1, &palette());
        let cell = buf.cell(0, 0).unwrap();
        assert_eq!(cell.cluster, "A");
        assert_eq!(
            cell.fg,
            TermColor::Rgb(PinColor::rgb(0xcd, 0x00, 0x00)),
            "scrollback keeps fg color (index 1 resolved to xterm red)"
        );
        assert!(cell.attrs.bold, "scrollback keeps bold");
    }

    #[test]
    fn projects_wide_head_and_trailer() {
        let screen = screen_from("世".as_bytes(), 6, 1);
        let buffer = project(&screen, &palette());
        assert_eq!(
            buffer.cell(0, 0).unwrap().width,
            pinion_core::CellWidth::Wide
        );
        assert_eq!(
            buffer.cell(1, 0).unwrap().width,
            pinion_core::CellWidth::Trailer
        );
    }

    #[test]
    fn projects_cursor_position() {
        let screen = screen_from(b"abc", 10, 1);
        let buffer = project(&screen, &palette());
        assert_eq!(buffer.cursor().col, 3);
        assert_eq!(buffer.cursor().row, 0);
    }

    /// The DECSCUSR blink MODE reaches pinion (R1425 / PINION-PR75), which is what makes the cursor
    /// actually blink — pinion runs the phase clock and gates the overlay on it. The phase itself is
    /// never in the projection, so `visible` stays pure DECTCEM: a blinking cursor is `visible` on
    /// both halves of its blink.
    ///
    /// REVERT-PROOF: dropping the `with_blink` chain leaves the shape and `visible` assertions
    /// passing and fails the blink one — the axis the whole slice exists for.
    #[test]
    fn projects_the_cursor_blink_mode() {
        let steady = project(&screen_from(b"\x1b[2 q", 4, 1), &palette());
        assert!(!steady.cursor().blink, "DECSCUSR 2 is a steady block");

        let blinking = project(&screen_from(b"\x1b[5 q", 4, 1), &palette());
        assert!(blinking.cursor().blink, "DECSCUSR 5 is a blinking bar");
        assert_eq!(blinking.cursor().shape, PinCursorShape::Bar);
        assert!(
            blinking.cursor().visible,
            "the blink is a mode; visibility stays DECTCEM's alone",
        );
        assert!(
            blinking.cursor().shown_this_phase(true) && !blinking.cursor().shown_this_phase(false),
            "pinion's own gate reads the mode: drawn on the visible phase only",
        );
    }

    /// The `OSC 12` cursor colour reaches pinion (R1424 / PINION-PR74) — and only when the child
    /// actually set one. Unset projects `None`, which pinion draws from the cell the cursor sits on
    /// (the reverse-video default); an `OSC 112` reset returns to it.
    ///
    /// REVERT-PROOF: chaining the palette's cursor colour unconditionally — the shape a
    /// non-`Option` palette field would force — passes the set assertion and fails both `None`
    /// ones, painting every default cursor the seed grey.
    #[test]
    fn projects_the_osc_12_cursor_color_only_when_set() {
        // The palette is the SSOT for this colour, so drive it through the emulator that owns it.
        let mut em = Emulator::new(4, 1);
        assert_eq!(
            project(em.screen(), em.palette()).cursor().cursor_color,
            None,
            "nothing set: the cursor takes its cell's colour",
        );

        em.advance(b"\x1b]12;rgb:00/00/ff\x1b\\");
        assert_eq!(
            project(em.screen(), em.palette()).cursor().cursor_color,
            Some(PinColor::rgb(0x00, 0x00, 0xff)),
            "a set colour is projected as an absolute Color, not a palette reference",
        );

        em.advance(b"\x1b]112\x1b\\");
        assert_eq!(
            project(em.screen(), em.palette()).cursor().cursor_color,
            None,
            "the reset returns the cursor to its cell-derived colour",
        );
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
                .filter_map(|c| buf.cell(c, 0).map(|cell| cell.cluster.to_string()))
                .collect::<String>()
                .trim_end()
                .to_owned()
        };
        let row1 = |buf: &GridBuffer| {
            (0..buf.cols())
                .filter_map(|c| buf.cell(c, 1).map(|cell| cell.cluster.to_string()))
                .collect::<String>()
                .trim_end()
                .to_owned()
        };
        // offset 0 == live (rows d, e), identical to project().
        let live = project_scrolled(&screen, 0, &palette());
        assert_eq!((row0(&live), row1(&live)), ("d".into(), "e".into()));
        // offset 1: one scrollback line ("c") on top, visible "d" below.
        let up1 = project_scrolled(&screen, 1, &palette());
        assert_eq!((row0(&up1), row1(&up1)), ("c".into(), "d".into()));
        // offset 3: top of history ("a", "b").
        let up3 = project_scrolled(&screen, 3, &palette());
        assert_eq!((row0(&up3), row1(&up3)), ("a".into(), "b".into()));
        // Clamp: a larger offset cannot scroll past the oldest line.
        let up99 = project_scrolled(&screen, 99, &palette());
        assert_eq!((row0(&up99), row1(&up99)), ("a".into(), "b".into()));
    }

    /// A stale positive offset against empty scrollback (history cleared, or
    /// alt-screen) clamps to the live view and KEEPS the cursor — not a
    /// cursor-less window.
    #[test]
    fn project_scrolled_clamps_stale_offset_to_live_with_cursor() {
        let screen = screen_from(b"abc", 10, 2);
        assert_eq!(screen.scrollback_len(), 0);
        let scrolled = project_scrolled(&screen, 7, &palette()); // stale offset, no history
        let live = project(&screen, &palette());
        assert_eq!(scrolled.cursor().col, live.cursor().col);
        assert!(
            scrolled.cursor().visible,
            "the live cursor is present after the clamp"
        );
    }

    /// A narrow preedit is spliced in at the cursor (col 2 after "ab"),
    /// underlined, leaving the committed cells to its left intact.
    #[test]
    fn overlay_preedit_underlines_narrow_text_at_the_cursor() {
        let screen = screen_from(b"ab", 10, 1);
        let buffer = overlay_preedit(project(&screen, &palette()), "x");
        assert_eq!(
            buffer.cell(0, 0).unwrap().cluster,
            "a",
            "committed text is preserved"
        );
        assert_eq!(buffer.cell(1, 0).unwrap().cluster, "b");
        let composed = buffer.cell(2, 0).unwrap(); // cursor sat at col 2
        assert_eq!(composed.cluster, "x");
        assert!(
            composed.attrs.underline.is_on(),
            "the preedit is underlined (composing marker)"
        );
    }

    /// The underline style axis and SGR-58 underline colour survive the
    /// projection into pinion's `TermCell` — the two separate slots (style on
    /// `CellAttrs`, colour on `TermCell`) that make an editor's red curly LSP
    /// error renderable rather than flattened to a plain rule.
    #[test]
    fn projection_carries_underline_style_and_color() {
        // curly (4:3) + red (58:2) underline through the real SGR parser
        let screen = screen_from(b"\x1b[4:3;58:2::255:0:0mE", 10, 1);
        let buffer = project(&screen, &palette());
        let cell = buffer.cell(0, 0).unwrap();
        assert_eq!(cell.attrs.underline, PinUnderlineStyle::Curly);
        assert_eq!(
            cell.underline_color,
            Some(TermColor::Rgb(PinColor::rgb(0xff, 0x00, 0x00)))
        );
    }

    /// A plain underline projects with no underline colour (the SGR-59
    /// default), so pinion draws the rule in the cell's own foreground.
    #[test]
    fn projection_default_underline_color_is_none() {
        let screen = screen_from(b"\x1b[4mU", 10, 1);
        let buffer = project(&screen, &palette());
        let cell = buffer.cell(0, 0).unwrap();
        assert_eq!(cell.attrs.underline, PinUnderlineStyle::Single);
        assert_eq!(cell.underline_color, None);
    }

    /// A wide (Hangul) preedit syllable expands to pinion's head + trailer pair,
    /// occupying the two cells its committed form will.
    #[test]
    fn overlay_preedit_expands_a_wide_syllable_to_head_and_trailer() {
        let screen = screen_from(b"ab", 10, 1);
        let buffer = overlay_preedit(project(&screen, &palette()), "한");
        let head = buffer.cell(2, 0).unwrap();
        assert_eq!(head.cluster, "한");
        assert_eq!(head.width, pinion_core::CellWidth::Wide);
        assert!(head.attrs.underline.is_on());
        assert_eq!(
            buffer.cell(3, 0).unwrap().width,
            pinion_core::CellWidth::Trailer
        );
    }

    /// An empty preedit (no active composition) is a no-op — the live view is
    /// byte-identical to the bare projection.
    #[test]
    fn overlay_preedit_empty_is_a_no_op() {
        let screen = screen_from(b"ab", 10, 1);
        let plain = project(&screen, &palette());
        let overlaid = overlay_preedit(project(&screen, &palette()), "");
        for col in 0..plain.cols() {
            assert_eq!(
                plain.cell(col, 0).unwrap().cluster,
                overlaid.cell(col, 0).unwrap().cluster
            );
        }
    }

    /// A wide preedit whose trailer would run off the row edge is clipped WHOLE —
    /// never written as a malformed Narrow-tagged wide cell (the M2 fix).
    #[test]
    fn overlay_preedit_clips_a_wide_syllable_at_the_row_edge() {
        let screen = screen_from(b"abc", 4, 1); // 4 cols, cursor lands on the last column (3)
        assert_eq!(project(&screen, &palette()).cursor().col, 3);
        let buffer = overlay_preedit(project(&screen, &palette()), "한");
        let last = buffer.cell(3, 0).unwrap();
        assert_ne!(
            last.cluster, "한",
            "the wide head is clipped at the edge, not written"
        );
        assert_ne!(
            last.width,
            pinion_core::CellWidth::Wide,
            "no lone Narrow-tagged wide head"
        );
    }

    /// The `project_scrolled` + `overlay_preedit` COMPOSITION a display client runs:
    /// a scrolled history window drops the cursor, so the overlay self-gates off and
    /// the preedit appears NOWHERE — while the live window (offset 0) shows it. This
    /// is the end-to-end fact the GUI's `build_pane_scene` relies on (topology B: the
    /// host projects, the client overlays); it used to live in a sprag-host test over
    /// the removed all-in-one pane builder, and belongs here with the two primitives.
    #[test]
    fn overlay_preedit_no_op_on_a_scrolled_history_window() {
        let screen = screen_from(b"a\r\nb\r\nc\r\nd\r\ne", 4, 2); // 3 rows into history
        assert_eq!(screen.scrollback_len(), 3);
        let contains_han = |buf: &GridBuffer| {
            (0..buf.cols())
                .any(|c| (0..buf.rows()).any(|r| buf.cell(c, r).is_some_and(|x| x.cluster == "한")))
        };
        // Live (offset 0): the composition overlays the preedit at the cursor.
        let live = overlay_preedit(project_scrolled(&screen, 0, &palette()), "한");
        assert!(contains_han(&live), "the live window shows the preedit");
        // Scrolled (offset 1): the cursor is dropped, so the overlay self-gates off.
        let scrolled = overlay_preedit(project_scrolled(&screen, 1, &palette()), "한");
        assert!(
            !contains_han(&scrolled),
            "a scrolled history window shows no preedit"
        );
    }

    /// No visible cursor (a scrolled history window, or a DECTCEM-hidden cursor)
    /// is no compose anchor — the overlay is a no-op (the single S1 gate).
    #[test]
    fn overlay_preedit_no_op_without_a_visible_cursor() {
        let screen = screen_from(b"ab", 10, 1);
        let hidden = project(&screen, &palette()).with_cursor(GridCursor::new(
            2,
            0,
            PinCursorShape::Block,
            false,
        ));
        let out = overlay_preedit(hidden, "x");
        assert_ne!(
            out.cell(2, 0).unwrap().cluster,
            "x",
            "no overlay without a visible cursor"
        );
    }

    /// Match highlights RECOLOUR rather than invert, and only the spans given. The distinction is
    /// the reason the function exists: an inverted match inside an inverted selection would cancel
    /// to no highlight at all, over exactly the cells the user is looking at.
    #[test]
    fn overlay_matches_recolours_only_the_spans() {
        let screen = screen_from(b"ab cd", 10, 1);
        let buf = project(&screen, &palette());
        // The projection has already resolved every cell's colour against the palette, so an
        // untouched cell's bg is whatever it projected to — captured here rather than assumed.
        let untouched = buf.cell(2, 0).map(|c| c.bg);
        let out = overlay_matches(
            buf,
            &[(0, 3, 2)],
            TermColor::Indexed(0),
            TermColor::Indexed(6),
        );
        assert_eq!(out.cell(3, 0).map(|c| c.bg), Some(TermColor::Indexed(6)));
        assert_eq!(out.cell(4, 0).map(|c| c.bg), Some(TermColor::Indexed(6)));
        assert_eq!(
            out.cell(2, 0).map(|c| c.bg),
            untouched,
            "a cell outside the span keeps its own colour",
        );
        assert!(
            out.cell(3, 0).is_some_and(|c| !c.attrs.reverse),
            "a match is recoloured, NOT inverted",
        );
    }

    /// A span on a row the buffer does not have, or a zero-width one, paints nothing rather than
    /// panicking or smearing the first row.
    #[test]
    fn overlay_matches_ignores_out_of_range_and_empty_spans() {
        let screen = screen_from(b"ab", 10, 1);
        let buf = project(&screen, &palette());
        let before = buf.clone();
        let out = overlay_matches(
            buf,
            &[(99, 0, 2), (0, 0, 0)],
            TermColor::Indexed(0),
            TermColor::Indexed(6),
        );
        assert_eq!(
            out.cell(0, 0).map(|c| c.bg),
            before.cell(0, 0).map(|c| c.bg),
        );
    }

    /// A same-row selection inverts exactly the selected columns; a cell already in
    /// reverse video toggles back to normal under the band (xterm-style invert).
    #[test]
    fn overlay_selection_inverts_the_selected_span() {
        let screen = screen_from(b"abcdef", 10, 1);
        let buf = overlay_selection(project(&screen, &palette()), (1, 0), (3, 0)); // cols 1..=3
        assert!(!buf.cell(0, 0).unwrap().attrs.reverse, "col 0 outside");
        assert!(buf.cell(1, 0).unwrap().attrs.reverse, "col 1 selected");
        assert!(buf.cell(3, 0).unwrap().attrs.reverse, "col 3 selected");
        assert!(!buf.cell(4, 0).unwrap().attrs.reverse, "col 4 outside");
        // SGR 7 (reverse) cell toggles back to normal inside the selection.
        let rev = screen_from(b"\x1b[7mX", 4, 1);
        assert!(project(&rev, &palette()).cell(0, 0).unwrap().attrs.reverse);
        let sel = overlay_selection(project(&rev, &palette()), (0, 0), (0, 0));
        assert!(
            !sel.cell(0, 0).unwrap().attrs.reverse,
            "a reverse cell inverts to normal under the selection"
        );
    }

    /// A multi-row selection is linear: first row from the start col to the row end,
    /// whole middle rows, the last row up to the end col.
    #[test]
    fn overlay_selection_spans_rows_linearly() {
        let screen = screen_from(b"aaaa\r\nbbbb\r\ncccc", 4, 3);
        let buf = overlay_selection(project(&screen, &palette()), (2, 0), (1, 2));
        // Row 0: cols 2,3 selected; 0,1 not.
        assert!(!buf.cell(1, 0).unwrap().attrs.reverse);
        assert!(buf.cell(2, 0).unwrap().attrs.reverse);
        // Row 1 (middle): fully selected.
        assert!(buf.cell(0, 1).unwrap().attrs.reverse);
        assert!(buf.cell(3, 1).unwrap().attrs.reverse);
        // Row 2: cols 0,1 selected; 2,3 not.
        assert!(buf.cell(1, 2).unwrap().attrs.reverse);
        assert!(!buf.cell(2, 2).unwrap().attrs.reverse);
    }

    // ---- OSC 8 hyperlink projection ----

    /// A linked cell projects to a `HyperlinkId` that resolves through the
    /// buffer's interning table to the OSC-8 URI; an unlinked cell carries none.
    #[test]
    fn projection_interns_a_hyperlink_and_resolves_its_uri() {
        let screen = screen_from(b"\x1b]8;;https://ok\x1b\\LINK\x1b]8;;\x1b\\ x", 20, 1);
        let buf = project(&screen, &palette());
        let id = buf
            .cell(0, 0)
            .unwrap()
            .hyperlink
            .expect("col 0 (L) is linked");
        assert_eq!(buf.hyperlink(id).unwrap().uri, "https://ok");
        assert!(
            buf.cell(5, 0).unwrap().hyperlink.is_none(),
            "the cell past the closed link is unlinked"
        );
    }

    /// A `id=`-tagged link that appears in two non-adjacent runs interns to ONE
    /// table entry (the pointer dedup collapses its shared `Arc`), so both runs
    /// project to the same id — one logical link a client highlights together.
    #[test]
    fn projection_groups_a_same_id_link_to_one_table_entry() {
        let screen = screen_from(
            b"\x1b]8;id=g;http://x\x1b\\A\x1b]8;;\x1b\\ \x1b]8;id=g;http://x\x1b\\B\x1b]8;;\x1b\\",
            20,
            1,
        );
        let buf = project(&screen, &palette());
        let a = buf.cell(0, 0).unwrap().hyperlink.expect("A linked");
        let b = buf.cell(2, 0).unwrap().hyperlink.expect("B linked");
        assert_eq!(a, b, "a same-id link interns to a single id");
        assert_eq!(buf.hyperlink(a).unwrap().id.as_deref(), Some("g"));
    }

    /// Two ANONYMOUS links to the same URI project to DISTINCT ids — the dedup
    /// is by `Arc` pointer, not URI value, so separate runs stay separate.
    #[test]
    fn projection_keeps_anonymous_same_uri_links_distinct() {
        let screen = screen_from(
            b"\x1b]8;;http://x\x1b\\A\x1b]8;;\x1b\\ \x1b]8;;http://x\x1b\\B\x1b]8;;\x1b\\",
            20,
            1,
        );
        let buf = project(&screen, &palette());
        let a = buf.cell(0, 0).unwrap().hyperlink.expect("A linked");
        let b = buf.cell(2, 0).unwrap().hyperlink.expect("B linked");
        assert_ne!(
            a, b,
            "two anonymous links get distinct ids even with an equal URI"
        );
        assert_eq!(buf.hyperlink(a).unwrap().uri, buf.hyperlink(b).unwrap().uri);
    }

    /// The hover overlay reverse-videos the WHOLE id-group — a wrapped link's
    /// cells on both rows light together; unlinked cells stay put; `None` no-ops.
    #[test]
    fn hyperlink_hover_overlay_lights_the_whole_id_group() {
        // A 6-char link on a 4-col screen wraps: "ABCD" on row 0, "EF" on row 1.
        let screen = screen_from(b"\x1b]8;;http://w\x1b\\ABCDEF\x1b]8;;\x1b\\", 4, 3);
        let buf = project(&screen, &palette());
        let id = buf.cell(0, 0).unwrap().hyperlink.expect("row0 linked");
        assert_eq!(
            buf.cell(1, 1).unwrap().hyperlink,
            Some(id),
            "the wrap put the same-id link on row 1"
        );
        let base = buf.cell(0, 0).unwrap().attrs.reverse;
        let lit = overlay_hyperlink_hover(project(&screen, &palette()), Some(id));
        assert_eq!(
            lit.cell(0, 0).unwrap().attrs.reverse,
            !base,
            "row 0 link cell lit"
        );
        assert_eq!(
            lit.cell(1, 1).unwrap().attrs.reverse,
            !base,
            "row 1 wrap cell lit (whole id-group)"
        );
        // A None hover is a no-op.
        let plain = overlay_hyperlink_hover(project(&screen, &palette()), None);
        assert_eq!(
            plain.cell(0, 0).unwrap().attrs.reverse,
            base,
            "no hover = unchanged"
        );
    }
}
