//! The termwiz-backed terminal emulator.
//!
//! DESIGN.md §4: termwiz is the embedded verified escape parser; this
//! module is the sprag-owned state machine that turns the parsed
//! [`Action`] stream into a [`Screen`]. termwiz tokenizes the bytes
//! (the max-risk part, delegated to a verified library); sprag decides
//! what each semantic action does to the grid.
//!
//! Scope is the walking-skeleton subset (DESIGN.md §5): print with
//! autowrap and wide-cluster handling, CR/LF/BS/HT, SGR pen, cursor
//! moves, erase-in-line/display, and alternate-screen + cursor-visibility
//! private modes. Unhandled sequences are ignored (see [`Emulator::advance`]).

use termwiz::cell::{Blink, Intensity, Underline};
use termwiz::color::ColorSpec;
use termwiz::escape::csi::{
    CSI, Cursor as CsiCursor, DecPrivateMode, DecPrivateModeCode, Edit, EraseInDisplay,
    EraseInLine, Mode, Sgr,
};
use termwiz::escape::parser::Parser;
use termwiz::escape::{Action, ControlCode};

use crate::port::{
    Attrs, Cell, Color, Cursor, CursorShape, InputModes, Rgb, Screen, ScreenKind, VtPort, Width,
    char_columns,
};

/// A terminal emulator: feed PTY bytes via [`VtPort::advance`], read the
/// resulting [`Screen`] via [`VtPort::screen`].
pub struct Emulator {
    parser: Parser,
    /// The active screen (main, or alternate while a fullscreen app runs).
    screen: Screen,
    /// The saved main screen while the alternate screen is active.
    saved_main: Option<Screen>,
    cols: u16,
    rows: u16,
    // Cursor + pen state the screen does not itself track.
    col: u16,
    row: u16,
    cursor_visible: bool,
    cursor_shape: CursorShape,
    fg: Color,
    bg: Color,
    attrs: Attrs,
    /// Input modes set by the child (DECCKM, …) that the key encoder
    /// reads; tracked here, exposed via [`VtPort::input_modes`].
    input_modes: InputModes,
    /// Monotonic damage stamp, bumped on every row-mutating action.
    generation: u64,
    /// `true` between a resize and the next batch of bytes — the window in which a
    /// line editor (bash/readline) redraws its wrapped prompt on `SIGWINCH`. Two
    /// behaviours change in that window so the redraw stays one clean logical line
    /// that collapses on a later widen — the way a reflowing terminal (vte/
    /// `gnome-terminal`) handles the same bytes:
    ///
    /// * an explicit `CR LF` (which readline emits at a width the line exactly
    ///   fills, instead of relying on autowrap) is treated as a SOFT wrap by
    ///   [`Self::control`], not a hard line end — otherwise it splits the prompt
    ///   into separate logical lines that cannot rejoin, leaving per-width copies
    ///   stacked as ghosts (the resize-stale accumulation);
    /// * the redraw's leading erase-in-line ([`Self::edit`]) clears the whole
    ///   wrapped active line, not just the cursor's row, so the stale tail left by
    ///   the prior width does not survive as a growing leftover in the input.
    ///
    /// Cleared once the redraw batch is applied (see [`VtPort::advance`]).
    in_resize_redraw: bool,
}

impl Emulator {
    /// A fresh emulator with a blank `cols x rows` main screen.
    #[must_use]
    pub fn new(cols: u16, rows: u16) -> Self {
        Self {
            parser: Parser::new(),
            screen: Screen::new(cols.max(1), rows.max(1)),
            saved_main: None,
            cols: cols.max(1),
            rows: rows.max(1),
            col: 0,
            row: 0,
            cursor_visible: true,
            cursor_shape: CursorShape::Block,
            fg: Color::Default,
            bg: Color::Default,
            attrs: Attrs::default(),
            input_modes: InputModes::default(),
            generation: 0,
            in_resize_redraw: false,
        }
    }

    fn next_gen(&mut self) -> u64 {
        self.generation += 1;
        self.generation
    }

    /// Apply one parsed action to the grid.
    fn apply(&mut self, action: Action) {
        match action {
            Action::Print(ch) => self.print_str(&ch.to_string()),
            Action::PrintString(s) => self.print_str(&s),
            Action::Control(code) => self.control(code),
            Action::CSI(csi) => self.csi(csi),
            // Esc, OSC, device-control, APC: not part of the skeleton subset.
            _ => {}
        }
    }

    fn control(&mut self, code: ControlCode) {
        match code {
            ControlCode::LineFeed | ControlCode::VerticalTab | ControlCode::FormFeed => {
                // A line feed normally ends this row's logical line (a hard break).
                // During a line editor's resize redraw (`in_resize_redraw`), an
                // explicit `CR LF` is instead how readline continues a wrapped prompt
                // at a width the line exactly fills — semantically a soft wrap, so the
                // redraw stays one logical line whose per-width copies collapse on a
                // later widen instead of stacking as ghosts.
                self.screen.set_wrapped(self.row, self.in_resize_redraw);
                self.line_feed();
            }
            ControlCode::CarriageReturn => self.col = 0,
            ControlCode::Backspace => self.col = self.col.saturating_sub(1),
            ControlCode::HorizontalTab => {
                // Advance to the next 8-column tab stop, clamped to width.
                let next = ((self.col / 8) + 1) * 8;
                self.col = next.min(self.cols.saturating_sub(1));
            }
            _ => {}
        }
    }

    fn csi(&mut self, csi: CSI) {
        match csi {
            CSI::Sgr(sgr) => self.sgr(sgr),
            CSI::Cursor(c) => self.cursor_op(c),
            CSI::Edit(e) => self.edit(e),
            CSI::Mode(m) => self.mode(m),
            _ => {}
        }
    }

    fn sgr(&mut self, sgr: Sgr) {
        match sgr {
            Sgr::Reset => {
                self.fg = Color::Default;
                self.bg = Color::Default;
                self.attrs = Attrs::default();
            }
            Sgr::Intensity(Intensity::Bold) => {
                self.attrs.bold = true;
                self.attrs.dim = false;
            }
            Sgr::Intensity(Intensity::Half) => {
                self.attrs.dim = true;
                self.attrs.bold = false;
            }
            Sgr::Intensity(Intensity::Normal) => {
                self.attrs.bold = false;
                self.attrs.dim = false;
            }
            Sgr::Underline(u) => self.attrs.underline = u != Underline::None,
            Sgr::Blink(b) => self.attrs.blink = b != Blink::None,
            Sgr::Italic(on) => self.attrs.italic = on,
            Sgr::Inverse(on) => self.attrs.reverse = on,
            Sgr::Invisible(on) => self.attrs.hidden = on,
            Sgr::StrikeThrough(on) => self.attrs.strikethrough = on,
            Sgr::Foreground(c) => self.fg = conv_color(c),
            Sgr::Background(c) => self.bg = conv_color(c),
            // Font, Overline, UnderlineColor, VerticalAlign: ignored.
            _ => {}
        }
    }

    fn cursor_op(&mut self, c: CsiCursor) {
        let max_col = self.cols.saturating_sub(1);
        let max_row = self.rows.saturating_sub(1);
        match c {
            CsiCursor::Up(n) => self.row = self.row.saturating_sub(clamp_count(n)),
            CsiCursor::Down(n) => self.row = (self.row + clamp_count(n)).min(max_row),
            CsiCursor::Left(n) => self.col = self.col.saturating_sub(clamp_count(n)),
            CsiCursor::Right(n) => self.col = (self.col + clamp_count(n)).min(max_col),
            CsiCursor::Position { line, col } => {
                self.row = zero_based_u16(line.as_zero_based()).min(max_row);
                self.col = zero_based_u16(col.as_zero_based()).min(max_col);
            }
            CsiCursor::CharacterAbsolute(c) | CsiCursor::CharacterPositionAbsolute(c) => {
                self.col = zero_based_u16(c.as_zero_based()).min(max_col);
            }
            CsiCursor::LinePositionAbsolute(n) => {
                self.row = zero_based_u16(n.saturating_sub(1)).min(max_row);
            }
            CsiCursor::NextLine(n) => {
                self.row = (self.row + clamp_count(n)).min(max_row);
                self.col = 0;
            }
            CsiCursor::PrecedingLine(n) => {
                self.row = self.row.saturating_sub(clamp_count(n));
                self.col = 0;
            }
            _ => {}
        }
    }

    fn edit(&mut self, e: Edit) {
        match e {
            Edit::EraseInLine(mode) => {
                let g = self.next_gen();
                let (start, end) = match mode {
                    EraseInLine::EraseToEndOfLine => (self.col, self.cols),
                    EraseInLine::EraseToStartOfLine => (0, self.col.saturating_add(1)),
                    EraseInLine::EraseLine => (0, self.cols),
                };
                let row = self.row;
                for c in start..end.min(self.cols) {
                    self.screen.set_cell(c, row, Cell::blank(), g);
                }
                // During a resize redraw, the editor's leading erase-in-line clears
                // its current row before reprinting the whole wrapped line. Extend it
                // to the line's soft-wrapped continuation rows too (read BEFORE the
                // wrap flag below is cleared), so the stale tail left by the prior
                // width — which the reprint may only partly cover — does not survive
                // as a growing leftover. Bounded to the active line's continuation,
                // and only while a redraw is in flight.
                if self.in_resize_redraw && end >= self.cols {
                    // Walk the wrap chain to the line's last continuation row FIRST
                    // (clearing a row drops its wrap flag, which would otherwise cut
                    // the walk short), then clear those rows.
                    let mut last = row;
                    while last + 1 < self.rows && self.screen.wrapped(last) {
                        last += 1;
                    }
                    for r in (row + 1)..=last {
                        self.screen.clear_row(r, g);
                    }
                }
                // Erasing to the right margin truncates the line, so it no
                // longer soft-wraps onto the next row.
                if end >= self.cols {
                    self.screen.set_wrapped(row, false);
                }
            }
            Edit::EraseInDisplay(mode) => {
                let g = self.next_gen();
                match mode {
                    EraseInDisplay::EraseToEndOfDisplay => {
                        let row = self.row;
                        for c in self.col..self.cols {
                            self.screen.set_cell(c, row, Cell::blank(), g);
                        }
                        for r in (row + 1)..self.rows {
                            self.screen.clear_row(r, g);
                        }
                    }
                    EraseInDisplay::EraseToStartOfDisplay => {
                        for r in 0..self.row {
                            self.screen.clear_row(r, g);
                        }
                        let row = self.row;
                        for c in 0..=self.col.min(self.cols.saturating_sub(1)) {
                            self.screen.set_cell(c, row, Cell::blank(), g);
                        }
                    }
                    EraseInDisplay::EraseDisplay => {
                        for r in 0..self.rows {
                            self.screen.clear_row(r, g);
                        }
                    }
                    // ED-3: drop the retained scrollback (R16 models it).
                    EraseInDisplay::EraseScrollback => self.screen.clear_scrollback(),
                }
            }
            // DeleteLine/InsertLine/ScrollUp/Repeat etc.: not in the subset.
            _ => {}
        }
    }

    fn mode(&mut self, m: Mode) {
        match m {
            Mode::SetDecPrivateMode(DecPrivateMode::Code(code)) => match code {
                DecPrivateModeCode::ShowCursor => self.cursor_visible = true,
                DecPrivateModeCode::ApplicationCursorKeys => {
                    self.input_modes.application_cursor_keys = true;
                }
                DecPrivateModeCode::ClearAndEnableAlternateScreen
                | DecPrivateModeCode::EnableAlternateScreen
                | DecPrivateModeCode::OptEnableAlternateScreen => self.enter_alt(),
                _ => {}
            },
            Mode::ResetDecPrivateMode(DecPrivateMode::Code(code)) => match code {
                DecPrivateModeCode::ShowCursor => self.cursor_visible = false,
                DecPrivateModeCode::ApplicationCursorKeys => {
                    self.input_modes.application_cursor_keys = false;
                }
                DecPrivateModeCode::ClearAndEnableAlternateScreen
                | DecPrivateModeCode::EnableAlternateScreen
                | DecPrivateModeCode::OptEnableAlternateScreen => self.exit_alt(),
                _ => {}
            },
            _ => {}
        }
    }

    fn enter_alt(&mut self) {
        if self.saved_main.is_none() {
            let mut alt = Screen::new(self.cols, self.rows);
            alt.set_kind(ScreenKind::Alternate);
            let main = std::mem::replace(&mut self.screen, alt);
            self.saved_main = Some(main);
            self.col = 0;
            self.row = 0;
        }
    }

    fn exit_alt(&mut self) {
        if let Some(main) = self.saved_main.take() {
            self.screen = main;
            let cur = self.screen.cursor();
            self.col = cur.col.min(self.cols.saturating_sub(1));
            self.row = cur.row.min(self.rows.saturating_sub(1));
        }
    }

    /// Print one or more graphemes, advancing the cursor with autowrap.
    fn print_str(&mut self, s: &str) {
        // Char-level is sufficient for the skeleton; ZWJ emoji clusters
        // are a known gap (DESIGN.md §5 — logged, not silently capped).
        for ch in s.chars() {
            let w = char_columns(ch); // the one width authority (port::char_columns)
            if w == 0 {
                // Combining mark: merge into the previous cell if possible.
                self.merge_combining(ch);
                continue;
            }
            let cell_w = w as u16;
            if self.col + cell_w > self.cols {
                // Autowrap: this row's logical line continues onto the next.
                self.screen.set_wrapped(self.row, true);
                self.col = 0;
                self.line_feed();
            }
            let g = self.next_gen();
            let head = Cell {
                cluster: ch.to_string(),
                fg: self.fg,
                bg: self.bg,
                attrs: self.attrs,
                width: if cell_w == 2 {
                    Width::Wide
                } else {
                    Width::Narrow
                },
            };
            let (col, row) = (self.col, self.row);
            if cell_w == 2 && col + 1 < self.cols {
                self.screen
                    .set_cell(col + 1, row, Cell::trailer_for(&head), g);
            }
            self.screen.set_cell(col, row, head, g);
            self.col += cell_w;
        }
    }

    fn merge_combining(&mut self, ch: char) {
        if self.col == 0 {
            return;
        }
        let (col, row) = (self.col - 1, self.row);
        if let Some(prev) = self.screen.cell(col, row) {
            let mut merged = prev.clone();
            merged.cluster.push(ch);
            let g = self.next_gen();
            self.screen.set_cell(col, row, merged, g);
        }
    }

    fn line_feed(&mut self) {
        if self.row + 1 >= self.rows {
            let g = self.next_gen();
            self.screen.scroll_up(g);
        } else {
            self.row += 1;
        }
    }

    /// Publish the tracked cursor into the screen (call after a batch).
    fn sync_cursor(&mut self) {
        self.screen.set_cursor(Cursor {
            col: self.col.min(self.cols.saturating_sub(1)),
            row: self.row.min(self.rows.saturating_sub(1)),
            shape: self.cursor_shape,
            visible: self.cursor_visible,
        });
    }
}

impl VtPort for Emulator {
    fn advance(&mut self, bytes: &[u8]) {
        // Parse the whole batch into actions first, then apply: this
        // avoids borrowing `self.parser` and `self` simultaneously.
        let actions = self.parser.parse_as_vec(bytes);
        for action in actions {
            self.apply(action);
        }
        // The line editor's resize redraw arrives as the first batch after a resize;
        // its soft-wrap / erase reinterpretations (see `in_resize_redraw`) end here.
        self.in_resize_redraw = false;
        self.sync_cursor();
    }

    fn resize(&mut self, cols: u16, rows: u16) {
        let cols = cols.max(1);
        let rows = rows.max(1);
        // Reflow rewraps the visible MAIN screen's logical lines to the new width
        // (the alt screen / degenerate sizes fall back to a verbatim copy inside
        // `reflowed`). A fresh damage stamp marks every re-laid-out row.
        let g = self.next_gen();
        let reflowed = self.screen.reflowed(cols, rows, g);
        // Adopt the cursor re-derived from the reflow (clamped for the verbatim
        // alt-screen path, a no-op for the in-bounds reflow path).
        self.col = reflowed.cursor().col.min(cols - 1);
        self.row = reflowed.cursor().row.min(rows - 1);
        self.screen = reflowed;
        if let Some(main) = &self.saved_main {
            self.saved_main = Some(main.reflowed(cols, rows, g));
        }
        self.cols = cols;
        self.rows = rows;
        // The next batch of bytes is the line editor's `SIGWINCH` redraw; apply the
        // soft-wrap / erase reinterpretations to it (see `in_resize_redraw`). Only
        // the MAIN screen runs a line editor; a fullscreen app owns the alt screen.
        self.in_resize_redraw = self.screen.screen_kind() == ScreenKind::Main;
        self.sync_cursor();
    }

    fn screen(&self) -> &Screen {
        &self.screen
    }

    fn input_modes(&self) -> InputModes {
        self.input_modes
    }
}

/// Convert a termwiz `ColorSpec` to the port's `Color`.
fn conv_color(spec: ColorSpec) -> Color {
    match spec {
        ColorSpec::Default => Color::Default,
        ColorSpec::PaletteIndex(i) => Color::Indexed(i),
        ColorSpec::TrueColor(srgba) => {
            let (r, g, b, _a) = srgba.to_srgb_u8();
            Color::Rgb(Rgb::new(r, g, b))
        }
    }
}

/// A movement count: termwiz may emit 0 for an omitted parameter; ANSI
/// treats that as 1.
fn clamp_count(n: u32) -> u16 {
    u16::try_from(n.max(1)).unwrap_or(u16::MAX)
}

fn zero_based_u16(n: u32) -> u16 {
    u16::try_from(n).unwrap_or(u16::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cluster(em: &Emulator, col: u16, row: u16) -> &str {
        em.screen()
            .cell(col, row)
            .map_or("", |c| c.cluster.as_str())
    }

    #[test]
    fn prints_and_advances_cursor() {
        let mut em = Emulator::new(10, 3);
        em.advance(b"hi");
        assert_eq!(cluster(&em, 0, 0), "h");
        assert_eq!(cluster(&em, 1, 0), "i");
        assert_eq!(em.screen().cursor().col, 2);
        assert_eq!(em.screen().cursor().row, 0);
    }

    #[test]
    fn sgr_sets_color_and_attrs() {
        let mut em = Emulator::new(10, 1);
        em.advance(b"\x1b[1;31mA");
        let cell = em.screen().cell(0, 0).unwrap();
        assert_eq!(cell.fg, Color::Indexed(1));
        assert!(cell.attrs.bold);
    }

    #[test]
    fn truecolor_foreground() {
        let mut em = Emulator::new(10, 1);
        em.advance(b"\x1b[38;2;10;20;30mX");
        assert_eq!(
            em.screen().cell(0, 0).unwrap().fg,
            Color::Rgb(Rgb::new(10, 20, 30))
        );
    }

    #[test]
    fn wide_cjk_head_and_trailer() {
        let mut em = Emulator::new(10, 1);
        em.advance("世".as_bytes());
        let head = em.screen().cell(0, 0).unwrap();
        assert_eq!(head.cluster, "世");
        assert_eq!(head.width, Width::Wide);
        let trailer = em.screen().cell(1, 0).unwrap();
        assert_eq!(trailer.width, Width::Trailer);
        assert_eq!(em.screen().cursor().col, 2);
    }

    #[test]
    fn carriage_return_and_line_feed() {
        let mut em = Emulator::new(10, 3);
        em.advance(b"ab\r\nc");
        assert_eq!(cluster(&em, 0, 0), "a");
        assert_eq!(cluster(&em, 0, 1), "c");
        assert_eq!(em.screen().cursor().row, 1);
        assert_eq!(em.screen().cursor().col, 1);
    }

    #[test]
    fn erase_line_clears_row() {
        let mut em = Emulator::new(10, 1);
        em.advance(b"abc\x1b[2K");
        for c in 0..3 {
            assert_eq!(cluster(&em, c, 0), " ");
        }
    }

    #[test]
    fn autowrap_to_next_row() {
        let mut em = Emulator::new(3, 2);
        em.advance(b"abcd");
        assert_eq!(cluster(&em, 0, 0), "a");
        assert_eq!(cluster(&em, 2, 0), "c");
        assert_eq!(cluster(&em, 0, 1), "d");
        assert_eq!(em.screen().cursor().row, 1);
    }

    #[test]
    fn alternate_screen_round_trip() {
        let mut em = Emulator::new(10, 2);
        em.advance(b"main");
        em.advance(b"\x1b[?1049h");
        assert_eq!(em.screen().screen_kind(), ScreenKind::Alternate);
        assert_eq!(cluster(&em, 0, 0), " ");
        em.advance(b"\x1b[?1049l");
        assert_eq!(em.screen().screen_kind(), ScreenKind::Main);
        assert_eq!(cluster(&em, 0, 0), "m");
    }

    #[test]
    fn scroll_on_overflow_keeps_last_line() {
        let mut em = Emulator::new(4, 2);
        em.advance(b"a\r\nb\r\nc");
        // After two line feeds past a 2-row screen, the top scrolls away.
        assert_eq!(cluster(&em, 0, 0), "b");
        assert_eq!(cluster(&em, 0, 1), "c");
    }

    #[test]
    fn damage_generation_advances_on_write() {
        let mut em = Emulator::new(4, 2);
        let g0 = em.screen().row_generation(0).unwrap();
        em.advance(b"x");
        assert!(em.screen().row_generation(0).unwrap() > g0);
    }

    #[test]
    fn application_cursor_keys_mode_defaults_off() {
        let em = Emulator::new(4, 2);
        assert!(!em.input_modes().application_cursor_keys);
    }

    #[test]
    fn decckm_set_and_reset_tracked() {
        let mut em = Emulator::new(4, 2);
        // DECSET 1 (ESC [ ? 1 h) enables application cursor keys.
        em.advance(b"\x1b[?1h");
        assert!(em.input_modes().application_cursor_keys);
        // DECRST 1 (ESC [ ? 1 l) restores normal cursor keys.
        em.advance(b"\x1b[?1l");
        assert!(!em.input_modes().application_cursor_keys);
    }

    // ----- B1: soft-wrap continuation metadata (`Screen::wrapped`) -----

    #[test]
    fn autowrap_marks_the_row_wrapped() {
        let mut em = Emulator::new(4, 3);
        em.advance(b"abcdef"); // 6 chars in 4 cols -> row0 "abcd" wraps to row1 "ef"
        assert!(em.screen().wrapped(0), "row 0 soft-wrapped onto row 1");
        assert!(!em.screen().wrapped(1), "row 1 did not wrap");
    }

    #[test]
    fn hard_linefeed_clears_the_wrapped_flag() {
        let mut em = Emulator::new(4, 3);
        em.advance(b"abcde"); // wraps -> wrapped[0] = true
        assert!(em.screen().wrapped(0));
        em.advance(b"\x1b[H\n"); // home to row 0, then a hard line feed
        assert!(
            !em.screen().wrapped(0),
            "a hard line feed ends the logical line"
        );
    }

    #[test]
    fn erase_line_clears_the_wrapped_flag() {
        let mut em = Emulator::new(4, 3);
        em.advance(b"abcde"); // wraps -> wrapped[0] = true
        assert!(em.screen().wrapped(0));
        em.advance(b"\x1b[H\x1b[2K"); // home, then erase the whole line
        assert!(
            !em.screen().wrapped(0),
            "erasing the line drops the soft wrap"
        );
    }

    // ----- resize-redraw reinterpretation (`in_resize_redraw`) -----

    #[test]
    fn resize_redraw_crlf_is_a_soft_wrap() {
        // After a resize, the line editor's redraw uses an explicit CR LF to
        // continue a wrapped line at an exact-fill width; treat it as a soft wrap
        // so the redraw stays one logical line (collapses on a later widen).
        let mut em = Emulator::new(10, 4);
        em.advance(b"x");
        em.resize(10, 4); // arms the redraw window
        em.advance(b"\rAAAA\r\nBBBB"); // CR, content, CR LF (the wrap idiom), content
        assert!(
            em.screen().wrapped(0),
            "a CR LF inside the resize redraw is a soft wrap"
        );
    }

    #[test]
    fn normal_crlf_outside_a_redraw_is_a_hard_break() {
        // Without a preceding resize, the same CR LF ends the logical line — so
        // ordinary command output keeps its real line breaks.
        let mut em = Emulator::new(10, 4);
        em.advance(b"AAAA\r\nBBBB");
        assert!(
            !em.screen().wrapped(0),
            "a CR LF in normal output is a hard line break"
        );
    }

    #[test]
    fn redraw_window_ends_after_the_first_batch() {
        // The soft-wrap reinterpretation lasts only for the redraw batch; a CR LF in
        // a later batch is hard again.
        let mut em = Emulator::new(10, 4);
        em.resize(10, 4);
        em.advance(b"\rAAAA"); // first batch (the redraw) — window closes after it
        em.advance(b"BBBB\r\nCCCC"); // a later batch
        assert!(
            !em.screen().wrapped(0),
            "the redraw window closed; this CR LF is hard"
        );
    }

    #[test]
    fn resize_redraw_erase_clears_the_wrapped_continuation() {
        // The redraw's leading erase-in-line clears the whole wrapped active line,
        // not just the cursor's row, so the stale tail of the prior width is gone.
        let mut em = Emulator::new(4, 4);
        em.advance(b"abcdefgh"); // row0 "abcd" (wrapped) -> row1 "efgh"
        assert_eq!(em.screen().row_text(1), "efgh");
        em.resize(4, 4); // arms the window; cursor anchored to the line top (row 0)
        em.advance(b"\r\x1b[K"); // CR + erase-to-end-of-line at the line top
        assert_eq!(
            em.screen().row_text(1),
            "",
            "the wrapped continuation row was cleared too"
        );
    }

    // ----- B2: reflow on resize -----

    fn row(em: &Emulator, r: u16) -> String {
        em.screen().row_text(r)
    }

    #[test]
    fn reflow_rejoins_a_wrapped_line_when_widened() {
        let mut em = Emulator::new(4, 3);
        em.advance(b"abcdef"); // wraps: row0 "abcd" -> row1 "ef"
        assert!(em.screen().wrapped(0));
        em.resize(8, 3);
        // The logical line now fits in one row, cleanly rejoined.
        assert_eq!(row(&em, 0), "abcdef");
        assert!(
            !em.screen().wrapped(0),
            "no longer wrapped at the wider width"
        );
        assert_eq!(row(&em, 1), "", "the continuation row is gone");
        // Cursor preserved by logical position: after 'f'.
        assert_eq!((em.screen().cursor().col, em.screen().cursor().row), (6, 0));
    }

    #[test]
    fn reflow_rebreaks_a_line_when_narrowed() {
        let mut em = Emulator::new(8, 3);
        em.advance(b"abcdef"); // fits in one row at width 8
        assert!(!em.screen().wrapped(0));
        em.resize(4, 3);
        // The logical line re-breaks at the new margin.
        assert_eq!(row(&em, 0), "abcd");
        assert!(
            em.screen().wrapped(0),
            "row 0 soft-wraps at the narrow width"
        );
        assert_eq!(row(&em, 1), "ef");
        // The cursor anchors to the FIRST physical row of its logical line (row 0),
        // not the continuation row it wrapped onto — keeping a line editor's resize
        // redraw (CR + erase + reprint, which assumes the cursor is at the line's
        // top) overwriting in place instead of stacking. Pulled up from a lower row,
        // its column pins to 0 (the line start) rather than the natural `offset %
        // width`, which would slide as the line re-breaks at different widths; see
        // `Screen::reflowed`'s cursor-anchor note.
        assert_eq!((em.screen().cursor().col, em.screen().cursor().row), (0, 0));
    }

    #[test]
    fn reflow_keeps_natural_column_on_a_single_row_line() {
        // When the cursor's logical line still fits on ONE physical row after the
        // reflow, the anchor row IS the cursor's own row, so its natural column is
        // preserved (no pin-to-0) — the caret stays after the text, not at the start.
        let mut em = Emulator::new(4, 3);
        em.advance(b"abcdef"); // wraps at width 4: row0 "abcd" -> row1 "ef", cursor after 'f'
        em.resize(8, 3); // widen: the line rejoins onto a single row
        assert_eq!(
            (em.screen().cursor().col, em.screen().cursor().row),
            (6, 0),
            "single-row line keeps the cursor after the text, column intact"
        );
    }

    #[test]
    fn reflow_anchors_cursor_to_logical_line_top() {
        // A line that wraps to several physical rows: after a reflow the cursor must
        // sit on the line's FIRST physical row so a live shell's `SIGWINCH` redraw
        // (CR + erase-in-line + reprint, no cursor-up) overwrites the old prompt
        // rather than stacking a fresh copy below it (the resize-stale bug).
        let mut em = Emulator::new(12, 4);
        em.advance(b"abcdefghijkl"); // exactly fills row 0 at width 12
        em.advance(b"mnop"); // wraps onto row 1; cursor after 'p'
        assert!(em.screen().wrapped(0), "the logical line spans rows 0..1");
        em.resize(4, 6); // re-break the 16-glyph line to width 4 -> 4 physical rows
        assert_eq!(
            em.screen().cursor().row,
            0,
            "cursor anchors to the logical line's top row, not its wrapped bottom"
        );
    }

    #[test]
    fn reflow_round_trips_stably() {
        let mut em = Emulator::new(8, 3);
        em.advance(b"abcdef");
        let text = em.screen().full_text();
        em.resize(4, 4); // narrow (rewraps)
        em.resize(8, 3); // back to the original width
        assert_eq!(
            em.screen().full_text(),
            text,
            "widen∘narrow restores the text"
        );
    }

    #[test]
    fn reflow_skips_the_alternate_screen() {
        let mut em = Emulator::new(8, 2);
        em.advance(b"\x1b[?1049h"); // enter the alternate screen
        em.advance(b"abcdef"); // fits at width 8 on the alt screen
        em.resize(4, 2);
        // The alt screen is NOT reflowed (verbatim) — a fullscreen app owns its
        // layout. The verbatim copy truncates to the new width, no rejoin.
        assert_eq!(row(&em, 0), "abcd", "alt screen truncated, not rewrapped");
    }
}
