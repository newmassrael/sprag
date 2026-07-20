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
    CSI, Cursor as CsiCursor, CursorStyle, DecPrivateMode, DecPrivateModeCode, Edit,
    EraseInDisplay, EraseInLine, Mode, Sgr,
};
use termwiz::escape::osc::FinalTermSemanticPrompt;
use termwiz::escape::parser::Parser;
use termwiz::escape::{Action, ControlCode, Esc, EscCode, OperatingSystemCommand};

use crate::port::{
    Attrs, Cell, Color, Cursor, CursorShape, InputModes, Notification, PromptMark, Rgb, Screen,
    ScreenKind, VtPort, Width, char_columns,
};

/// The cursor state DECSC (`ESC 7` / `CSI s`) saves and DECRC (`ESC 8` / `CSI u`) restores:
/// position plus the SGR pen and cursor shape. Charset state is out of the emulator's subset, so
/// it is not part of the save (a documented bound, consistent with the rest of the skeleton).
#[derive(Clone, Copy)]
struct SavedCursor {
    col: u16,
    row: u16,
    fg: Color,
    bg: Color,
    attrs: Attrs,
    cursor_shape: CursorShape,
}

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
    /// The DECSTBM scroll region as an INCLUSIVE, 0-based row range
    /// `[scroll_top, scroll_bottom]`, defaulting to `[0, rows - 1]` = the whole screen.
    /// A line feed / IND at `scroll_bottom` scrolls the region up (its top line leaving);
    /// a reverse index (RI) at `scroll_top` scrolls it down; IL/DL/SU/SD act within it;
    /// rows outside the region stay put — the `less` / `vim` / tmux-status-bar split-region
    /// idiom. Reset to the full screen on resize and on every alt-screen transition (the
    /// region is screen-relative, and a fullscreen app sets its own after entering the alt
    /// screen). Origin mode (DECOM) is not modeled, so a DECSTBM homes the cursor to the
    /// SCREEN top-left, not the region's top — a documented bound consistent with the rest
    /// of the skeleton.
    scroll_top: u16,
    scroll_bottom: u16,
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
    /// The child's self-reported window TITLE (`OSC 0` / `OSC 2`), `None` until it
    /// sets one. Exposed via [`VtPort::title`]; a shell's `PROMPT_COMMAND` (or vim,
    /// ssh, tmux…) rewrites it continuously, so this is live state, NOT the spawn
    /// command label. Deliberately does NOT bump [`Self::generation`] — that stamp is
    /// ROW DAMAGE, and a title carries no cells; marking rows dirty for it would force
    /// needless cell re-render. The change still reaches consumers because the OSC
    /// bytes arrive as PTY output, which already fires the session's `on_dirty`.
    title: Option<String>,
    /// The most recent attention notification the child raised (`OSC 9` / `OSC
    /// 777;notify` / `OSC 99`), or `None`. Latched (last wins), exposed via
    /// [`VtPort::notification`]. Like [`Self::title`] it deliberately does NOT bump
    /// [`Self::generation`] — it carries no cells — and reaches consumers because the
    /// OSC bytes arrive as PTY output, which already fires the session's `on_dirty`.
    notification: Option<Notification>,
    /// Monotonic count of notifications raised (`0` before the first), exposed via
    /// [`VtPort::notification_seq`]. Bumped once per captured notification so a
    /// consumer can tell a NEW one from a re-read of the same latched payload.
    notification_seq: u64,
    /// Monotonic count of BELLs (`\a`, `ControlCode::Bell`) the child has rung (`0` before the
    /// first), exposed via [`VtPort::bell_seq`]. A bell is the tmux `monitor-bell` signal — a
    /// text-less "pay attention" ping, DISTINCT from a [`Notification`] (a desktop toast that
    /// carries text). It is kept as its own counter, NOT folded into [`Self::notification_seq`],
    /// so the two attention sources stay individually addressable (as tmux keeps its bell flag
    /// separate from activity); the multiplexer's attention marker sums them. Like the
    /// notification seq it does NOT bump [`Self::generation`] — a bell carries no cells — and
    /// reaches consumers because the byte arrives as PTY output, which fires the session's
    /// `on_dirty`. NB the `\a` that TERMINATES an OSC string (its `ST`) is consumed by the parser
    /// as part of the OSC, so only a bare BEL in the stream counts here.
    bell_seq: u64,
    /// The cursor position + pen saved by DECSC (`ESC 7` / `CSI s`) and restored by DECRC
    /// (`ESC 8` / `CSI u`), or `None` before any save. Saves the same set a terminal restores —
    /// position, SGR foreground/background/attributes, and cursor shape — so an app that saves,
    /// draws in a different pen, then restores comes back exactly where and how it was.
    saved_cursor: Option<SavedCursor>,
    /// The last GRAPHIC character printed, for REP (`CSI b` — REPEAT). `None` until one is printed
    /// or after an action that is not a plain print. Repeat re-emits this, so it tracks exactly
    /// what a bare `print` would repeat.
    last_print: Option<char>,
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
    ///
    /// WHY a window and not a purely structural signal: the editor's `CR LF` lands
    /// MID-row (a premature break, columns short of the margin), so it is
    /// byte-for-byte indistinguishable from a genuine newline — only the resize
    /// CONTEXT marks it as a soft wrap. (vte/`gnome-terminal` likewise rely on
    /// context, not a pending-wrap latch, and likewise show the break until widen.)
    ///
    /// Scope LIMITS (held in practice, honestly bounded): it assumes the editor's
    /// redraw is the first `advance` batch after the resize and fits in one batch.
    /// A redraw split across PTY reads, or unrelated output arriving first, would
    /// fall outside the window. Editor prompt redraws are small (< one read) and
    /// foreground at the prompt, so this holds for the real cases; widening the
    /// scope is deferred until a case is observed to need it.
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
            scroll_top: 0,
            scroll_bottom: rows.max(1) - 1,
            col: 0,
            row: 0,
            cursor_visible: true,
            cursor_shape: CursorShape::Block,
            fg: Color::Default,
            bg: Color::Default,
            attrs: Attrs::default(),
            input_modes: InputModes::default(),
            title: None,
            notification: None,
            notification_seq: 0,
            bell_seq: 0,
            saved_cursor: None,
            last_print: None,
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
            Action::OperatingSystemCommand(osc) => self.osc(&osc),
            Action::Esc(esc) => self.esc(esc),
            // Device-control (sixel), APC (Kitty graphics): not part of the subset.
            _ => {}
        }
    }

    /// The two-byte `ESC <final>` sequences in the subset:
    ///
    /// * DECSC (`ESC 7`) / DECRC (`ESC 8`) save and restore the cursor + pen — the same
    ///   save/restore the `CSI s` / `CSI u` forms drive ([`cursor_op`](Self::cursor_op)).
    /// * IND (`ESC D`, index) moves the cursor down one line, scrolling the region up at
    ///   the bottom margin — identical to a bare line feed ([`line_feed`](Self::line_feed)),
    ///   minus the carriage return.
    /// * RI (`ESC M`, reverse index) is the mirror: up one line, scrolling the region DOWN
    ///   at the top margin ([`reverse_index`](Self::reverse_index)).
    ///
    /// Every other ESC (charset selection, NEL, keypad modes) is out of the subset and
    /// dropped.
    fn esc(&mut self, esc: Esc) {
        if let Esc::Code(code) = esc {
            match code {
                EscCode::DecSaveCursorPosition => self.save_cursor(),
                EscCode::DecRestoreCursorPosition => self.restore_cursor(),
                EscCode::Index => self.line_feed(),
                EscCode::ReverseIndex => self.reverse_index(),
                _ => {}
            }
        }
    }

    /// RI (reverse index, `ESC M`): move the cursor UP one line. At the top margin this
    /// scrolls the region DOWN by one (a blank line opens at the top margin, the cursor
    /// stays put) — the mirror of a line feed at the bottom margin. Above the region (a
    /// cursor parked over a fixed header) it just steps up, stopping at the screen top.
    fn reverse_index(&mut self) {
        if self.row == self.scroll_top {
            let g = self.next_gen();
            self.screen
                .scroll_region_down(self.scroll_top, self.scroll_bottom, 1, g);
        } else if self.row > 0 {
            self.row -= 1;
        }
    }

    /// Reset the DECSTBM scroll region to the whole screen. The region is screen-relative,
    /// so a resize or an alt-screen transition (a fresh buffer) starts it at the full
    /// extent; an app that wants a sub-region sets one with DECSTBM afterwards.
    fn reset_scroll_region(&mut self) {
        self.scroll_top = 0;
        self.scroll_bottom = self.rows.saturating_sub(1);
    }

    /// DECSTBM (`CSI Pt ; Pb r`): set the scroll region to the inclusive, 0-based rows
    /// `[top, bottom]`. termwiz supplies defaults for omitted parameters (top 1, bottom the
    /// last row) as 1-based `OneBased`; the caller passes them already 0-based, `bottom`
    /// clamped here to the last row (termwiz's "big default" is `u32::MAX - 1`). A region
    /// needs at least two lines: an invalid one (`top >= bottom`) is IGNORED, leaving the
    /// margins and cursor unchanged (the VT100 rule). On a valid set the cursor homes to
    /// the screen top-left (origin mode is not modeled — see the field doc).
    fn set_scroll_region(&mut self, top: u32, bottom: u32) {
        let max_row = self.rows.saturating_sub(1);
        let top = u16::try_from(top).unwrap_or(u16::MAX).min(max_row);
        let bottom = u16::try_from(bottom).unwrap_or(u16::MAX).min(max_row);
        if top < bottom {
            self.scroll_top = top;
            self.scroll_bottom = bottom;
            self.row = 0;
            self.col = 0;
        }
    }

    /// Save the cursor position + pen (DECSC / `CSI s`).
    fn save_cursor(&mut self) {
        self.saved_cursor = Some(SavedCursor {
            col: self.col,
            row: self.row,
            fg: self.fg,
            bg: self.bg,
            attrs: self.attrs,
            cursor_shape: self.cursor_shape,
        });
    }

    /// Restore the cursor position + pen saved by DECSC (DECRC / `CSI u`). With no prior save the
    /// spec homes the cursor and resets the pen — the state a fresh save would hold — so a restore
    /// is always well-defined.
    fn restore_cursor(&mut self) {
        let saved = self.saved_cursor.unwrap_or(SavedCursor {
            col: 0,
            row: 0,
            fg: Color::Default,
            bg: Color::Default,
            attrs: Attrs::default(),
            cursor_shape: CursorShape::Block,
        });
        self.col = saved.col.min(self.cols.saturating_sub(1));
        self.row = saved.row.min(self.rows.saturating_sub(1));
        self.fg = saved.fg;
        self.bg = saved.bg;
        self.attrs = saved.attrs;
        self.cursor_shape = saved.cursor_shape;
    }

    /// Operating-system commands. Two families are in the subset:
    ///
    /// * the WINDOW-TITLE family — `OSC 0` (icon name AND window title) and `OSC 2`
    ///   (window title), plus termwiz's Sun-style spelling. `OSC 1` sets only the ICON
    ///   name — not a window title — so it is ignored.
    /// * the ATTENTION-NOTIFICATION family — `OSC 9` (iTerm2/xterm `SystemNotification`),
    ///   `OSC 777;notify;title;body` (urxvt), and `OSC 99` (kitty) — captured as a
    ///   [`Notification`] the multiplexer surfaces as "this pane wants attention".
    ///
    /// Every other OSC (hyperlinks, clipboard, colour queries) is dropped, per the
    /// skeleton contract.
    ///
    /// Child-controlled strings (the title, and each notification field) are CLAMPED
    /// ([`clamp_title`] / [`MAX_NOTIFICATION_BYTES`]): the underlying `vtparse` bounds
    /// the OSC parameter COUNT, not the payload BYTE length, so an uncapped store would
    /// let a hostile/buggy child buffer an arbitrarily large string that is then cloned
    /// every poll wake and shipped over the wire. This mirrors the `RAW_CAPTURE_CAP`
    /// bound on the sibling child-controlled buffer.
    fn osc(&mut self, osc: &OperatingSystemCommand) {
        match osc {
            OperatingSystemCommand::SetWindowTitle(t)
            | OperatingSystemCommand::SetWindowTitleSun(t)
            | OperatingSystemCommand::SetIconNameAndWindowTitle(t) => {
                self.title = Some(clamp_title(t));
            }
            // OSC 9 — the iTerm2/xterm growl notification: a single message, no title.
            // (termwiz routes `OSC 9;4;…` ConEmu progress to its own variant, so a
            // `SystemNotification` reaching here is always a genuine notification.)
            OperatingSystemCommand::SystemNotification(message) => {
                self.raise_notification(None, message);
            }
            // OSC 777 — urxvt's extension family; `notify` is its desktop notification,
            // `OSC 777 ; notify ; <title> ; <body>` (body optional). Any other urxvt
            // extension is ignored.
            OperatingSystemCommand::RxvtExtension(params) => {
                if let Some(kind) = params.first()
                    && kind == "notify"
                {
                    let title = params.get(1).map(String::as_str);
                    let body = params.get(2).map(String::as_str).unwrap_or("");
                    self.raise_notification(title, body);
                }
            }
            // OSC 99 — kitty's desktop-notification protocol. termwiz does not model it,
            // so it arrives as `Unspecified` raw params. [`parse_kitty_notification`]
            // handles the common single-chunk, unencoded case (see its doc for the
            // bounds); a multi-chunk or base64 payload is left uncaptured, not misparsed.
            OperatingSystemCommand::Unspecified(params) => {
                if let Some((title, body)) = parse_kitty_notification(params) {
                    self.raise_notification(title.as_deref(), &body);
                }
            }
            // OSC 133 (FinalTerm) shell-integration boundary marks — prompt / output / command
            // end. These drive jump-to-prompt + command-boundary detection (the modern-terminal
            // feature; tmux only passes them through).
            OperatingSystemCommand::FinalTermSemanticPrompt(prompt) => {
                self.shell_integration(prompt);
            }
            _ => {}
        }
    }

    /// Apply one OSC 133 (FinalTerm) semantic-prompt marker, attaching a [`PromptMark`] to the
    /// cursor's current row where one belongs. `A` (prompt start) and the fresh-line forms do a
    /// FinalTerm "fresh line" first — a `CR LF` unless already at the left margin — so the mark
    /// lands on a clean line. `B` (end of prompt / start of input) sets no row mark: at row
    /// granularity the user's input sits on the prompt row, so the command text is the rows from
    /// the prompt up to the output start (see [`PromptMark`]).
    fn shell_integration(&mut self, prompt: &FinalTermSemanticPrompt) {
        use FinalTermSemanticPrompt as F;
        match prompt {
            F::FreshLine => self.fresh_line(),
            // A — start of a prompt.
            F::FreshLineAndStartPrompt { .. } | F::StartPrompt(_) => {
                self.fresh_line();
                self.screen.set_mark(self.row, Some(PromptMark::Prompt));
            }
            // C — the command executed; its output starts here.
            F::MarkEndOfInputAndStartOfOutput { .. } => {
                self.screen.set_mark(self.row, Some(PromptMark::Output));
            }
            // D — the command finished, with the exit status the shell reported.
            F::CommandStatus { status, .. } => {
                self.screen
                    .set_mark(self.row, Some(PromptMark::CommandEnd(Some(*status))));
            }
            // D without a status (bare end-of-command), then a fresh line for the next prompt.
            F::MarkEndOfCommandWithFreshLine { .. } => {
                self.screen
                    .set_mark(self.row, Some(PromptMark::CommandEnd(None)));
                self.fresh_line();
            }
            // B — end of prompt / start of input: no row mark (see the doc above).
            F::MarkEndOfPromptAndStartOfInputUntilNextMarker
            | F::MarkEndOfPromptAndStartOfInputUntilEndOfLine => {}
        }
    }

    /// A FinalTerm "fresh line": if the cursor is not at the left margin, do the equivalent of a
    /// `CR LF` (column home + a region-aware line feed); otherwise nothing. Used by the OSC 133
    /// prompt / command-end markers so a mark lands at the head of a clean line.
    fn fresh_line(&mut self) {
        if self.col != 0 {
            self.col = 0;
            self.line_feed();
        }
    }

    /// Latch a captured attention notification (clamping both child-controlled fields)
    /// and bump the monotonic sequence so a consumer can tell it is new. Shared by every
    /// notification OSC so the clamp + counter live in ONE place.
    fn raise_notification(&mut self, title: Option<&str>, body: &str) {
        self.notification = Some(Notification {
            title: title.map(|t| clamp_bytes(t, MAX_NOTIFICATION_BYTES)),
            body: clamp_bytes(body, MAX_NOTIFICATION_BYTES),
        });
        self.notification_seq += 1;
    }

    fn control(&mut self, code: ControlCode) {
        match code {
            ControlCode::LineFeed | ControlCode::VerticalTab | ControlCode::FormFeed => {
                // A line feed ends this row's logical line (a hard break) — UNLESS it
                // is the editor's resize-redraw wrap idiom, where it CONTINUES the
                // line (a soft wrap). See `in_resize_redraw` for why.
                let soft_wrap = self.in_resize_redraw;
                self.screen.set_wrapped(self.row, soft_wrap);
                self.line_feed();
            }
            ControlCode::CarriageReturn => self.col = 0,
            ControlCode::Backspace => self.col = self.col.saturating_sub(1),
            ControlCode::HorizontalTab => {
                // Advance to the next 8-column tab stop, clamped to width.
                let next = ((self.col / 8) + 1) * 8;
                self.col = next.min(self.cols.saturating_sub(1));
            }
            // BEL (`\a`) — the tmux monitor-bell attention ping. Count it (a text-less attention
            // event); it does not touch the grid. See `bell_seq`.
            ControlCode::Bell => self.bell_seq += 1,
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
            // DECSC / DECRC in their `CSI s` / `CSI u` spelling (same save/restore as `ESC 7/8`).
            CsiCursor::SaveCursor => self.save_cursor(),
            CsiCursor::RestoreCursor => self.restore_cursor(),
            // DECSCUSR — the cursor SHAPE (block / underline / bar); blink is not modeled, so the
            // steady and blinking variants of each shape map to the same shape.
            CsiCursor::CursorStyle(style) => self.cursor_shape = cursor_shape_of(style),
            // DECSTBM — set the top/bottom scroll margins (`SetLeftAndRightMargins`, DECSLRM,
            // stays out of the subset).
            CsiCursor::SetTopAndBottomMargins { top, bottom } => {
                self.set_scroll_region(top.as_zero_based(), bottom.as_zero_based());
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
                // A line editor's resize redraw opens with erase-to-end-of-line
                // (`ESC [ K`) at the active line's head, then reprints the whole
                // wrapped line. Clear that line's stale continuation rows too (one
                // atomic, invariant-safe op on the `Screen`), so the prior width's
                // tail — which the reprint may only partly cover — does not linger.
                // Scoped to that exact idiom: only `EraseToEndOfLine`, only during a
                // redraw (`in_resize_redraw`); a plain erase touches one row.
                if self.in_resize_redraw && matches!(mode, EraseInLine::EraseToEndOfLine) {
                    self.screen.clear_soft_wrap_continuation(row, g);
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
            // ICH — insert n blanks at the cursor, shifting the rest of the row right.
            Edit::InsertCharacter(n) => {
                let g = self.next_gen();
                self.screen
                    .insert_cells(self.col, self.row, clamp_count(n), g);
            }
            // DCH — delete n cells at the cursor, shifting the rest of the row left.
            Edit::DeleteCharacter(n) => {
                let g = self.next_gen();
                self.screen
                    .delete_cells(self.col, self.row, clamp_count(n), g);
            }
            // ECH — blank n cells at the cursor in place (no shift).
            Edit::EraseCharacter(n) => {
                let g = self.next_gen();
                self.screen
                    .erase_cells(self.col, self.row, clamp_count(n), g);
            }
            // REP — reprint the last graphic char n times (a no-op before any print).
            Edit::Repeat(n) => {
                if let Some(ch) = self.last_print {
                    let repeated: String =
                        std::iter::repeat_n(ch, clamp_count(n) as usize).collect();
                    self.print_str(&repeated);
                }
            }
            // IL — insert n blank lines at the cursor, within the scroll region: rows from
            // the cursor down to the bottom margin shift down, the tail falling past the
            // margin. A no-op when the cursor is outside the region (the VT100 rule). The
            // active position moves to the line home (column 0) per ECMA-48.
            Edit::InsertLine(n) => {
                if self.row >= self.scroll_top && self.row <= self.scroll_bottom {
                    let g = self.next_gen();
                    self.screen
                        .scroll_region_down(self.row, self.scroll_bottom, clamp_count(n), g);
                    self.col = 0;
                }
            }
            // DL — delete n lines at the cursor, within the scroll region: rows below shift
            // up to the cursor, blanks opening at the bottom margin. A no-op outside the
            // region; column homes to 0. DL is an EDIT, so it never feeds the scrollback —
            // even a DL at row 0 removes the line rather than scrolling it off the top.
            Edit::DeleteLine(n) => {
                if self.row >= self.scroll_top && self.row <= self.scroll_bottom {
                    let g = self.next_gen();
                    self.screen.scroll_region_up(
                        self.row,
                        self.scroll_bottom,
                        clamp_count(n),
                        false, // an edit, not an output-flow scroll: no scrollback
                        g,
                    );
                    self.col = 0;
                }
            }
            // SU — scroll the region up n lines (data moves up); the cursor does not move.
            // An output-flow scroll, so a top-anchored region feeds the scrollback.
            Edit::ScrollUp(n) => {
                let g = self.next_gen();
                self.screen.scroll_region_up(
                    self.scroll_top,
                    self.scroll_bottom,
                    clamp_count(n),
                    true,
                    g,
                );
            }
            // SD — scroll the region down n lines (data moves down); the cursor does not
            // move. A down scroll discards the bottom line, never the top, so no scrollback.
            Edit::ScrollDown(n) => {
                let g = self.next_gen();
                self.screen.scroll_region_down(
                    self.scroll_top,
                    self.scroll_bottom,
                    clamp_count(n),
                    g,
                );
            }
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
            // The alt screen is a fresh buffer; it starts with the full-screen region.
            self.reset_scroll_region();
        }
    }

    fn exit_alt(&mut self) {
        if let Some(main) = self.saved_main.take() {
            self.screen = main;
            let cur = self.screen.cursor();
            self.col = cur.col.min(self.cols.saturating_sub(1));
            self.row = cur.row.min(self.rows.saturating_sub(1));
            // DECSTBM is not part of the saved cursor state; the restored main screen
            // resumes with the full-screen region (a well-behaved app reset it on exit).
            self.reset_scroll_region();
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
            // Remember the last graphic char for REP (`CSI b`).
            self.last_print = Some(ch);
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

    /// Move the cursor down one line (IND / the LF part of a line feed). At the bottom
    /// margin this scrolls the scroll region up by one — for the default full-screen region
    /// that is the ordinary "output flows off the top into scrollback" scroll. Below the
    /// bottom margin (a cursor parked over a fixed footer) it advances until the last row,
    /// then stops; it never scrolls a region it is not the bottom of.
    fn line_feed(&mut self) {
        if self.row == self.scroll_bottom {
            let g = self.next_gen();
            self.screen
                .scroll_region_up(self.scroll_top, self.scroll_bottom, 1, true, g);
        } else if self.row + 1 < self.rows {
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
        // The scroll region was defined against the old geometry; a resize returns it to
        // the full new screen (apps re-issue DECSTBM if they still want a sub-region).
        self.reset_scroll_region();
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

    fn title(&self) -> Option<&str> {
        self.title.as_deref()
    }

    fn notification(&self) -> Option<&Notification> {
        self.notification.as_ref()
    }

    fn notification_seq(&self) -> u64 {
        self.notification_seq
    }

    fn bell_seq(&self) -> u64 {
        self.bell_seq
    }
}

/// Parse a kitty `OSC 99` desktop notification from termwiz's raw `Unspecified` params
/// (`[b"99", b"<metadata>", b"<payload>", …]`), returning `(title, body)` or `None` when it
/// is not an `OSC 99` or carries no capturable text.
///
/// Kitty's form is `OSC 99 ; <metadata> ; <payload>`, where `<metadata>` is `k=v:k=v` pairs.
/// This handles the COMMON single-chunk case and reads two keys:
///
/// * `p` — payload type: `title` (kitty's default) or `body`; other types (`icon`, `close`,
///   `buttons`, …) are not text to show, so they are dropped.
/// * `e` — encoding: `1` means the payload is base64. sprag has no base64 decoder in this
///   layer, so an encoded payload is dropped rather than shown as gibberish.
///
/// BOUNDS (honestly limited, not misrepresented): a MULTI-CHUNK notification (`d=0`, streamed
/// across several `OSC 99`s) is NOT reassembled — each chunk is read independently, so a body
/// split across chunks yields only its first piece; `i`/`d`/actions are ignored. These are the
/// advanced-protocol tail; the single unencoded chunk is what shells and CLIs emit in practice.
fn parse_kitty_notification(params: &[Vec<u8>]) -> Option<(Option<String>, String)> {
    // Only OSC 99; anything else in `Unspecified` is some other unhandled OSC.
    if params.first().map(Vec::as_slice) != Some(b"99".as_slice()) {
        return None;
    }
    let metadata = params.get(1).map(Vec::as_slice).unwrap_or(b"");
    // The payload is everything after the second `;`; termwiz split it on `;`, so rejoin
    // (a plain-text payload may itself contain a semicolon).
    if params.len() < 3 {
        return None;
    }
    let payload_bytes = params[2..].join(&b';');
    let payload = String::from_utf8_lossy(&payload_bytes);

    let mut payload_type = "title"; // kitty's default when `p` is absent.
    let mut base64 = false;
    for pair in metadata.split(|&b| b == b':') {
        let mut kv = pair.splitn(2, |&b| b == b'=');
        let key = kv.next().unwrap_or(b"");
        let value = kv.next().unwrap_or(b"");
        match key {
            b"p" => {
                payload_type = match value {
                    b"body" => "body",
                    b"title" => "title",
                    _ => return None, // icon / close / buttons / … : nothing to display.
                };
            }
            b"e" if value == b"1" => base64 = true,
            _ => {}
        }
    }
    if base64 {
        return None; // encoded payload: not decoded in this layer (see the doc bound).
    }
    match payload_type {
        "body" => Some((None, payload.into_owned())),
        // Default / explicit title: the heading, with no separate body in this chunk.
        _ => Some((Some(payload.into_owned()), String::new())),
    }
}

/// Upper bound on a stored child window title. A title is a single taskbar / titlebar
/// line, so a few KiB is generous; the cap exists to stop a hostile or runaway child from
/// growing an unbounded `String` (see [`Emulator::osc`]). Bytes, not chars — the truncation
/// respects a UTF-8 boundary.
const MAX_TITLE_BYTES: usize = 2048;

/// Upper bound on a stored notification field (title or body). A notification is a short
/// desktop toast, so the same few-KiB budget as a window title is generous; the cap exists
/// for the same reason (a child-controlled string stored, cloned per poll wake, and shipped
/// over the wire must be bounded — see [`Emulator::osc`]).
const MAX_NOTIFICATION_BYTES: usize = 2048;

/// Clamp a child-set title to [`MAX_TITLE_BYTES`].
fn clamp_title(t: &str) -> String {
    clamp_bytes(t, MAX_TITLE_BYTES)
}

/// Clamp a child-controlled `String` to `max` BYTES, truncating on a char boundary so the
/// stored value stays valid UTF-8. Most values are far under the cap and clone as-is. Shared
/// by the window title and the notification fields — both are unbounded child input.
fn clamp_bytes(s: &str, max: usize) -> String {
    if s.len() <= max {
        return s.to_owned();
    }
    let mut end = max;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    s[..end].to_owned()
}

/// Map a termwiz DECSCUSR [`CursorStyle`] to the port's [`CursorShape`]. Blink is not modeled, so
/// each shape's steady and blinking variants collapse to the same shape; `Default` is a block (the
/// power-on default).
fn cursor_shape_of(style: CursorStyle) -> CursorShape {
    match style {
        CursorStyle::Default | CursorStyle::BlinkingBlock | CursorStyle::SteadyBlock => {
            CursorShape::Block
        }
        CursorStyle::BlinkingUnderline | CursorStyle::SteadyUnderline => CursorShape::Underline,
        CursorStyle::BlinkingBar | CursorStyle::SteadyBar => CursorShape::Bar,
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

    /// `OSC 2` (window title) and `OSC 0` (icon name AND window title) both set the
    /// title; the latest write wins (a shell rewrites it on every prompt).
    #[test]
    fn osc_0_and_2_set_the_window_title() {
        let mut em = Emulator::new(8, 2);
        assert_eq!(em.title(), None, "no title until the child sets one");

        em.advance(b"\x1b]2;vim README\x07");
        assert_eq!(em.title(), Some("vim README"));

        em.advance(b"\x1b]0;coin@host:~\x07");
        assert_eq!(em.title(), Some("coin@host:~"), "latest OSC wins");
    }

    /// `OSC 1` sets only the ICON name — not a window title — so it must NOT be
    /// mistaken for one (the whole point of matching the variants, not the OSC code).
    #[test]
    fn osc_1_icon_name_does_not_set_the_window_title() {
        let mut em = Emulator::new(8, 2);
        em.advance(b"\x1b]2;real title\x07");
        em.advance(b"\x1b]1;icon-only\x07");
        assert_eq!(
            em.title(),
            Some("real title"),
            "OSC 1 must not overwrite it"
        );
    }

    /// A child-controlled title is CLAMPED to `MAX_TITLE_BYTES` (vtparse bounds the OSC
    /// param count, not the byte length, so a hostile/runaway child could otherwise buffer
    /// an unbounded title). The truncation lands on a UTF-8 char boundary — here a `é`
    /// (2 bytes) straddling the cap must not split into an invalid `String`.
    #[test]
    fn a_hostile_oversized_title_is_clamped_on_a_char_boundary() {
        let mut em = Emulator::new(8, 2);
        // A title of many `é` (2 bytes each), well over the 2048-byte cap.
        let payload = "é".repeat(4000); // 8000 bytes
        em.advance(format!("\x1b]2;{payload}\x07").as_bytes());
        let title = em.title().expect("title set");
        assert!(
            title.len() <= MAX_TITLE_BYTES,
            "clamped to the cap ({} <= {MAX_TITLE_BYTES})",
            title.len(),
        );
        assert!(
            title.chars().all(|c| c == 'é'),
            "truncated on a char boundary — no split/replacement char",
        );
        // A title UNDER the cap is stored verbatim (the common path).
        em.advance(b"\x1b]2;short\x07");
        assert_eq!(em.title(), Some("short"));
    }

    /// A title carries NO cells, so it must not stamp ROW DAMAGE — else every prompt
    /// (which rewrites the title) would force a needless cell re-render. Consumers
    /// still learn of it: the OSC bytes are PTY output, which fires `on_dirty`.
    #[test]
    fn setting_the_title_does_not_bump_the_damage_generation() {
        let mut em = Emulator::new(8, 2);
        let g0 = em.screen().row_generation(0).unwrap();
        em.advance(b"\x1b]2;no damage\x07");
        assert_eq!(em.title(), Some("no damage"));
        assert_eq!(
            em.screen().row_generation(0).unwrap(),
            g0,
            "a title-only OSC leaves row damage untouched",
        );
    }

    /// `OSC 9` (iTerm2/xterm) raises a body-only notification and bumps the sequence, so a
    /// consumer can tell a new one arrived.
    #[test]
    fn osc_9_raises_a_body_only_notification() {
        let mut em = Emulator::new(8, 2);
        assert_eq!(em.notification(), None, "none until the child raises one");
        assert_eq!(em.notification_seq(), 0);

        em.advance(b"\x1b]9;build finished\x07");
        let n = em.notification().expect("notification set");
        assert_eq!(n.title, None, "OSC 9 carries no title");
        assert_eq!(n.body, "build finished");
        assert_eq!(em.notification_seq(), 1, "the sequence bumped once");

        // A second one latches over the first and bumps the sequence again.
        em.advance(b"\x1b]9;tests passed\x07");
        assert_eq!(
            em.notification().unwrap().body,
            "tests passed",
            "latest wins"
        );
        assert_eq!(em.notification_seq(), 2);
    }

    /// `OSC 777;notify;<title>;<body>` (urxvt) raises a titled notification; a non-`notify`
    /// urxvt extension raises nothing (only the notification sub-command is in the subset).
    #[test]
    fn osc_777_notify_raises_a_titled_notification() {
        let mut em = Emulator::new(8, 2);
        em.advance(b"\x1b]777;notify;Build;done in 3s\x07");
        let n = em.notification().expect("notification set");
        assert_eq!(n.title.as_deref(), Some("Build"));
        assert_eq!(n.body, "done in 3s");
        assert_eq!(em.notification_seq(), 1);

        // A different urxvt extension (not `notify`) is ignored — no new notification.
        em.advance(b"\x1b]777;something;else\x07");
        assert_eq!(
            em.notification_seq(),
            1,
            "a non-notify OSC 777 raises nothing",
        );
    }

    /// `OSC 99` (kitty): the default single-chunk payload is the TITLE; an explicit
    /// `p=body` payload is the BODY. The advanced tail (base64 `e=1`, non-text `p`) captures
    /// nothing rather than misparsing.
    #[test]
    fn osc_99_kitty_notification_maps_the_payload_by_type() {
        let mut em = Emulator::new(8, 2);
        // No metadata ⇒ kitty's default p=title.
        em.advance(b"\x1b]99;;Attention needed\x07");
        let n = em.notification().expect("title notification");
        assert_eq!(n.title.as_deref(), Some("Attention needed"));
        assert_eq!(n.body, "", "a title-only chunk has no body");
        assert_eq!(em.notification_seq(), 1);

        // Explicit p=body.
        em.advance(b"\x1b]99;p=body;the message\x07");
        let n = em.notification().expect("body notification");
        assert_eq!(n.title, None);
        assert_eq!(n.body, "the message");
        assert_eq!(em.notification_seq(), 2);

        // A base64-encoded payload is NOT decoded here — it must not be shown as gibberish,
        // and it must not bump the sequence (nothing was captured).
        em.advance(b"\x1b]99;e=1;aGk=\x07");
        assert_eq!(
            em.notification_seq(),
            2,
            "an encoded payload is dropped, not misparsed",
        );
        // A non-text payload type (e.g. an icon) captures nothing either.
        em.advance(b"\x1b]99;p=icon;whatever\x07");
        assert_eq!(em.notification_seq(), 2, "a non-text p= is ignored");
    }

    /// A notification carries NO cells, so — like the title — it must not stamp ROW DAMAGE.
    /// It still reaches consumers because the OSC bytes are PTY output (which fires `on_dirty`).
    #[test]
    fn a_notification_does_not_bump_the_damage_generation() {
        let mut em = Emulator::new(8, 2);
        let g0 = em.screen().row_generation(0).unwrap();
        em.advance(b"\x1b]9;ping\x07");
        assert_eq!(em.notification().unwrap().body, "ping");
        assert_eq!(
            em.screen().row_generation(0).unwrap(),
            g0,
            "a notification OSC leaves row damage untouched",
        );
    }

    /// Both notification fields are child-controlled, so both are CLAMPED like the title —
    /// on a UTF-8 char boundary, so an oversized payload cannot store an invalid `String`.
    #[test]
    fn a_hostile_oversized_notification_is_clamped() {
        let mut em = Emulator::new(8, 2);
        let payload = "é".repeat(4000); // 8000 bytes, over the cap
        em.advance(format!("\x1b]777;notify;{payload};{payload}\x07").as_bytes());
        let n = em.notification().expect("notification set");
        let title = n.title.as_deref().expect("titled");
        assert!(title.len() <= MAX_NOTIFICATION_BYTES && n.body.len() <= MAX_NOTIFICATION_BYTES);
        assert!(
            title.chars().all(|c| c == 'é') && n.body.chars().all(|c| c == 'é'),
            "truncated on a char boundary — no split/replacement char",
        );
    }

    /// ICH (`CSI @`) inserts blanks at the cursor, shifting the rest of the row right; cells
    /// pushed past the right margin fall off.
    #[test]
    fn insert_character_shifts_the_row_right() {
        let mut em = Emulator::new(6, 1);
        em.advance(b"abcd"); // a b c d _ _
        em.advance(b"\x1b[1G"); // cursor to column 1 (CHA, 1-based)
        em.advance(b"\x1b[2@"); // ICH 2
        assert_eq!(cluster(&em, 0, 0), " ");
        assert_eq!(cluster(&em, 1, 0), " ");
        assert_eq!(cluster(&em, 2, 0), "a", "the row shifted right by 2");
        assert_eq!(cluster(&em, 5, 0), "d", "d rode to the right margin");
    }

    /// DCH (`CSI P`) deletes cells at the cursor, shifting the rest of the row left and blanking
    /// the vacated tail.
    #[test]
    fn delete_character_shifts_the_row_left() {
        let mut em = Emulator::new(6, 1);
        em.advance(b"abcdef");
        em.advance(b"\x1b[1G"); // column 1
        em.advance(b"\x1b[2P"); // DCH 2
        assert_eq!(cluster(&em, 0, 0), "c", "the row shifted left by 2");
        assert_eq!(cluster(&em, 3, 0), "f");
        assert_eq!(cluster(&em, 4, 0), " ", "the tail is blanked");
        assert_eq!(cluster(&em, 5, 0), " ");
    }

    /// ECH (`CSI X`) blanks cells at the cursor IN PLACE — unlike DCH, the cells to the right do
    /// not move.
    #[test]
    fn erase_character_blanks_in_place_without_shifting() {
        let mut em = Emulator::new(6, 1);
        em.advance(b"abcdef");
        em.advance(b"\x1b[3G"); // column 3 (0-based col 2)
        em.advance(b"\x1b[2X"); // ECH 2
        assert_eq!(cluster(&em, 1, 0), "b");
        assert_eq!(cluster(&em, 2, 0), " ", "erased in place");
        assert_eq!(cluster(&em, 3, 0), " ");
        assert_eq!(cluster(&em, 4, 0), "e", "cells to the right did NOT shift");
    }

    /// REP (`CSI b`) reprints the last graphic char n times; it is a no-op before any print.
    #[test]
    fn repeat_reprints_the_last_graphic_char() {
        let mut em = Emulator::new(6, 1);
        em.advance(b"x"); // print x (cursor now at column 1)
        em.advance(b"\x1b[3b"); // REP 3
        for c in 0..4 {
            assert_eq!(cluster(&em, c, 0), "x", "x plus 3 repeats");
        }
        assert_eq!(em.screen().cursor().col, 4);

        // REP before any print does nothing (no last graphic char).
        let mut fresh = Emulator::new(4, 1);
        fresh.advance(b"\x1b[3b");
        assert_eq!(cluster(&fresh, 0, 0), " ");
        assert_eq!(fresh.screen().cursor().col, 0);
    }

    /// DECSC / DECRC (`ESC 7` / `ESC 8`) save and restore the cursor POSITION and the SGR PEN, so
    /// a save-draw-elsewhere-restore round trip returns to the exact spot and colour.
    #[test]
    fn decsc_decrc_save_and_restore_the_cursor_and_pen() {
        let mut em = Emulator::new(10, 3);
        em.advance(b"\x1b[31mR"); // a RED 'R' at row0 col0 — capture what red maps to
        let red = em.screen().cell(0, 0).unwrap().fg;
        assert_ne!(red, Color::Default, "red is a non-default pen");

        em.advance(b"\x1b[2;5H"); // move to row 2 col 5 (0-based row1 col4), pen still red
        em.advance(b"\x1b7"); // DECSC — save pos + red pen
        em.advance(b"\x1b[1;1H\x1b[0m"); // home + reset the pen to default
        em.advance(b"\x1b8"); // DECRC — restore pos + pen
        em.advance(b"Z"); // print at the restored spot with the restored pen

        let z = em.screen().cell(4, 1).unwrap();
        assert_eq!(z.cluster, "Z", "restored the saved POSITION (row1 col4)");
        assert_eq!(
            z.fg, red,
            "restored the saved PEN (red), not the reset default"
        );
    }

    /// The `CSI s` / `CSI u` spelling of DECSC / DECRC drives the SAME save/restore.
    #[test]
    fn csi_s_and_u_save_and_restore_the_cursor() {
        let mut em = Emulator::new(10, 3);
        em.advance(b"\x1b[2;5H"); // row1 col4
        em.advance(b"\x1b[s"); // save
        em.advance(b"\x1b[1;1H"); // home
        em.advance(b"\x1b[u"); // restore
        em.advance(b"Q");
        assert_eq!(
            em.screen().cell(4, 1).unwrap().cluster,
            "Q",
            "CSI u restored the position"
        );
    }

    /// DECSCUSR (`CSI SP q`) sets the cursor SHAPE; blink is not modeled, so each shape's steady
    /// and blinking codes map to the same shape, and `0`/`1` are the block default.
    #[test]
    fn decscusr_sets_the_cursor_shape() {
        let mut em = Emulator::new(4, 1);
        assert_eq!(
            em.screen().cursor().shape,
            CursorShape::Block,
            "block by default"
        );
        em.advance(b"\x1b[4 q"); // steady underline
        assert_eq!(em.screen().cursor().shape, CursorShape::Underline);
        em.advance(b"\x1b[5 q"); // blinking bar
        assert_eq!(em.screen().cursor().shape, CursorShape::Bar);
        em.advance(b"\x1b[0 q"); // default -> block
        assert_eq!(em.screen().cursor().shape, CursorShape::Block);
    }

    /// The title survives an alt-screen round trip: it is emulator-level state, not a
    /// property of either screen buffer (a fullscreen app sets a title, then restores).
    #[test]
    fn title_survives_the_alt_screen_round_trip() {
        let mut em = Emulator::new(8, 2);
        em.advance(b"\x1b]2;editor\x07");
        em.advance(b"\x1b[?1049h");
        assert_eq!(em.title(), Some("editor"));
        em.advance(b"\x1b[?1049l");
        assert_eq!(em.title(), Some("editor"));
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

    // ----- scroll-region slice (DECSTBM + IL/DL/SU/SD + RI/IND) -----

    /// DECSTBM (`CSI Pt;Pb r`) confines a line feed's scroll to the margins: a line feed at
    /// the bottom margin scrolls only the region, leaving the rows above and below fixed.
    /// It also homes the cursor when the region is set.
    #[test]
    fn scroll_region_confines_the_line_feed_to_the_margins() {
        let mut em = Emulator::new(4, 6);
        em.advance(b"0\r\n1\r\n2\r\n3\r\n4\r\n5"); // rows 0..5 labelled
        em.advance(b"\x1b[2;4r"); // region 1-based [2,4] = 0-based [1,3]
        assert_eq!(
            (em.screen().cursor().col, em.screen().cursor().row),
            (0, 0),
            "DECSTBM homes the cursor",
        );
        em.advance(b"\x1b[4;1H\n"); // to the bottom margin (row 3), then a line feed
        assert_eq!(
            em.screen().row_text(0),
            "0",
            "the row above the region is fixed"
        );
        assert_eq!(em.screen().row_text(1), "2", "the region scrolled up");
        assert_eq!(em.screen().row_text(2), "3");
        assert_eq!(
            em.screen().row_text(3),
            "",
            "the bottom margin row is blank"
        );
        assert_eq!(
            em.screen().row_text(4),
            "4",
            "the rows below the region are fixed"
        );
        assert_eq!(em.screen().row_text(5), "5");
    }

    /// RI (`ESC M`) at the top margin scrolls the region DOWN (a blank opens at the top);
    /// IND (`ESC D`) at the bottom margin scrolls it UP. Both keep the cursor at the margin.
    #[test]
    fn reverse_index_and_index_scroll_the_region_at_its_margins() {
        let mut em = Emulator::new(4, 5);
        em.advance(b"0\r\n1\r\n2\r\n3\r\n4");
        em.advance(b"\x1b[2;4r"); // region 0-based [1,3]
        em.advance(b"\x1b[2;1H\x1bM"); // to the top margin (row 1), then RI
        assert_eq!(
            em.screen().row_text(1),
            "",
            "RI opened a blank at the top margin"
        );
        assert_eq!(em.screen().row_text(2), "1");
        assert_eq!(em.screen().row_text(3), "2");
        assert_eq!(em.screen().row_text(0), "0", "outside the region is fixed");
        assert_eq!(em.screen().row_text(4), "4");
        assert_eq!(em.screen().cursor().row, 1, "RI stays at the top margin");
        em.advance(b"\x1b[4;1H\x1bD"); // to the bottom margin (row 3), then IND
        assert_eq!(
            em.screen().row_text(1),
            "1",
            "IND scrolled the region back up"
        );
        assert_eq!(em.screen().row_text(2), "2");
        assert_eq!(em.screen().row_text(3), "");
        assert_eq!(
            em.screen().cursor().row,
            3,
            "IND stays at the bottom margin"
        );
    }

    /// Away from a margin, IND / RI just move the cursor — they do NOT scroll. (Revert
    /// proof for the margin condition: without it, these would scroll and mangle contents.)
    #[test]
    fn index_and_reverse_index_move_without_scrolling_off_the_margins() {
        let mut em = Emulator::new(4, 5);
        em.advance(b"0\r\n1\r\n2\r\n3\r\n4");
        em.advance(b"\x1b[2;4r"); // region [1,3]
        em.advance(b"\x1b[2;1H\x1bD"); // top margin -> IND -> down one, no scroll
        assert_eq!(em.screen().cursor().row, 2);
        assert_eq!(
            em.screen().row_text(1),
            "1",
            "no scroll: the region is unchanged"
        );
        assert_eq!(em.screen().row_text(2), "2");
        em.advance(b"\x1b[4;1H\x1bM"); // bottom margin -> RI -> up one, no scroll
        assert_eq!(em.screen().cursor().row, 2);
        assert_eq!(
            em.screen().row_text(3),
            "3",
            "no scroll: the region is unchanged"
        );
    }

    /// IL (`CSI L`) opens blank lines and DL (`CSI M`) removes them, both bounded by the
    /// scroll region and homing the cursor column (ECMA-48). Rows outside the region (a
    /// fixed footer here) never move.
    #[test]
    fn insert_and_delete_line_are_region_bounded_and_home_the_column() {
        let mut em = Emulator::new(4, 5);
        em.advance(b"0\r\n1\r\n2\r\n3\r\n4"); // rows 0..4
        em.advance(b"\x1b[1;4r"); // region 0-based [0,3]; row 4 is the fixed footer
        em.advance(b"\x1b[2;3H\x1b[L"); // to row 1 col 2, then IL 1
        assert_eq!(em.screen().cursor().col, 0, "IL homes the column");
        assert_eq!(em.screen().row_text(0), "0", "above the insert is fixed");
        assert_eq!(
            em.screen().row_text(1),
            "",
            "a blank line opened at the cursor"
        );
        assert_eq!(em.screen().row_text(2), "1");
        assert_eq!(
            em.screen().row_text(3),
            "2",
            "the line at the bottom margin fell off"
        );
        assert_eq!(
            em.screen().row_text(4),
            "4",
            "the footer below the region is fixed"
        );
        em.advance(b"\x1b[2;3H\x1b[M"); // to row 1 col 2, then DL 1
        assert_eq!(em.screen().cursor().col, 0, "DL homes the column");
        assert_eq!(
            em.screen().row_text(1),
            "1",
            "the blank was removed, rows moved up"
        );
        assert_eq!(em.screen().row_text(2), "2");
        assert_eq!(
            em.screen().row_text(3),
            "",
            "a blank opened at the bottom margin"
        );
        assert_eq!(em.screen().row_text(4), "4", "the footer stays fixed");
    }

    /// SU (`CSI S`) and SD (`CSI T`) scroll the region by n without moving the cursor.
    #[test]
    fn scroll_up_and_scroll_down_move_the_region_by_n() {
        let mut em = Emulator::new(4, 5);
        em.advance(b"0\r\n1\r\n2\r\n3\r\n4");
        em.advance(b"\x1b[2;4r"); // region [1,3]
        em.advance(b"\x1b[2S"); // SU 2
        assert_eq!(
            em.screen().row_text(1),
            "3",
            "the region scrolled up by two"
        );
        assert_eq!(em.screen().row_text(2), "");
        assert_eq!(em.screen().row_text(3), "");
        assert_eq!(em.screen().row_text(0), "0", "outside the region is fixed");
        assert_eq!(em.screen().row_text(4), "4");
        em.advance(b"\x1b[T"); // SD 1
        assert_eq!(
            em.screen().row_text(1),
            "",
            "SD opened a blank at the top margin"
        );
        assert_eq!(em.screen().row_text(2), "3");
    }

    /// The scrollback rule: only an output-flow scroll of a TOP-ANCHORED region reaches
    /// the scrollback. A mid-screen region does not (interior lines), and DL never does
    /// (an edit, not a scroll) — even at row 0 of a top-anchored region.
    #[test]
    fn only_a_top_anchored_output_scroll_feeds_the_scrollback() {
        let mut mid = Emulator::new(4, 5);
        mid.advance(b"0\r\n1\r\n2\r\n3\r\n4");
        mid.advance(b"\x1b[2;4r\x1b[S"); // mid-screen region [1,3], SU 1
        assert_eq!(
            mid.screen().scrollback_len(),
            0,
            "a mid-screen region scroll is not history",
        );

        let mut top = Emulator::new(4, 5);
        top.advance(b"0\r\n1\r\n2\r\n3\r\n4");
        top.advance(b"\x1b[1;4r\x1b[S"); // top-anchored region [0,3], SU 1
        assert_eq!(
            top.screen().scrollback_rows().collect::<Vec<_>>(),
            ["0"],
            "a top-anchored output scroll is history",
        );

        let mut del = Emulator::new(4, 5);
        del.advance(b"0\r\n1\r\n2\r\n3\r\n4");
        del.advance(b"\x1b[1;4r\x1b[1;1H\x1b[M"); // top-anchored region, home, DL 1
        assert_eq!(
            del.screen().scrollback_len(),
            0,
            "DL removes a line; it is not scrolled into history",
        );
    }

    /// An invalid DECSTBM (`top >= bottom`) is ignored — the margins and the cursor are
    /// left untouched (the region stays full-screen).
    #[test]
    fn an_invalid_scroll_region_is_ignored() {
        let mut em = Emulator::new(4, 5);
        em.advance(b"0\r\n1\r\n2\r\n3\r\n4");
        em.advance(b"\x1b[3;3H"); // move the cursor off home (row 2 col 2)
        em.advance(b"\x1b[4;2r"); // top 4 >= bottom 2: invalid
        assert_eq!(
            (em.screen().cursor().col, em.screen().cursor().row),
            (2, 2),
            "an invalid DECSTBM does not home the cursor",
        );
        em.advance(b"\x1b[5;1H\n"); // last row, line feed
        assert_eq!(
            em.screen().scrollback_rows().collect::<Vec<_>>(),
            ["0"],
            "the region is still full-screen, so the whole screen scrolled",
        );
    }

    /// A resize returns the scroll region to the full screen: a line feed at the LAST row
    /// then scrolls the whole screen (a stale sub-region would not scroll from there).
    #[test]
    fn a_resize_resets_the_scroll_region_to_full_screen() {
        let mut em = Emulator::new(4, 5);
        em.advance(b"\x1b[2;3r"); // set a sub-region [1,2]
        em.resize(4, 5); // resets the region
        em.advance(b"0\r\n1\r\n2\r\n3\r\n4"); // fills rows 0..4
        em.advance(b"\n"); // LF at the last row -> whole-screen scroll iff region is full
        assert_eq!(
            em.screen().row_text(0),
            "1",
            "the screen scrolled as a whole"
        );
        assert_eq!(em.screen().scrollback_rows().next().as_deref(), Some("0"));
    }

    /// IL / DL are no-ops when the cursor is outside the scroll region (the VT100 rule).
    #[test]
    fn insert_and_delete_line_outside_the_region_do_nothing() {
        let mut em = Emulator::new(4, 5);
        em.advance(b"0\r\n1\r\n2\r\n3\r\n4");
        em.advance(b"\x1b[2;3r"); // region [1,2]; rows 0, 3, 4 are outside
        em.advance(b"\x1b[5;1H\x1b[L\x1b[M"); // cursor to row 4 (outside), IL then DL
        for (r, want) in [(0u16, "0"), (1, "1"), (2, "2"), (3, "3"), (4, "4")] {
            assert_eq!(
                em.screen().row_text(r),
                want,
                "IL/DL outside the region changed nothing",
            );
        }
    }

    /// The real split-region idiom: a scrolling area above a fixed footer. Output flows and
    /// scrolls within the region; the footer never moves; the line that leaves a
    /// top-anchored region becomes scrollback.
    #[test]
    fn output_flows_within_the_region_leaving_a_footer_fixed() {
        let mut em = Emulator::new(6, 4);
        em.advance(b"\x1b[4;1Hstatus"); // paint a footer on the last row (row 3)
        em.advance(b"\x1b[1;3r"); // rows 0..2 scroll; row 3 is fixed
        em.advance(b"\x1b[1;1Haaa\r\nbbb\r\nccc\r\nddd"); // 4 lines into a 3-row region
        assert_eq!(em.screen().row_text(0), "bbb", "the region scrolled");
        assert_eq!(em.screen().row_text(1), "ccc");
        assert_eq!(em.screen().row_text(2), "ddd");
        assert_eq!(em.screen().row_text(3), "status", "the footer never moved");
        assert_eq!(
            em.screen().scrollback_rows().next().as_deref(),
            Some("aaa"),
            "the line that scrolled off the top-anchored region is history",
        );
    }

    // ----- BEL -> attention (tmux monitor-bell) -----

    /// A BARE bell (`\a`) bumps `bell_seq` (the tmux monitor-bell ping); it is kept apart from
    /// the notification and stamps no row damage. The `\a` that TERMINATES an OSC is that OSC's
    /// string terminator, consumed by the parser — so it must NOT count as a bell.
    #[test]
    fn a_bare_bell_bumps_bell_seq_apart_from_the_notification() {
        let mut em = Emulator::new(8, 2);
        assert_eq!(em.bell_seq(), 0, "no bell until one rings");

        em.advance(b"\x07"); // a bare BEL
        assert_eq!(em.bell_seq(), 1);
        em.advance(b"a\x07b\x07"); // two more bares, interleaved with prints
        assert_eq!(em.bell_seq(), 3, "each bare bell counts");
        assert_eq!(em.notification(), None, "a bell is not a notification");

        // The `\a` terminating an OSC (OSC 2 window title, ST = BEL) is not a bell.
        em.advance(b"\x1b]2;t\x07");
        assert_eq!(em.bell_seq(), 3, "an OSC-terminating BEL does not count");
        assert_eq!(em.title(), Some("t"), "the OSC still applied");

        // A bell carries no cells: a bell-only batch leaves row damage untouched.
        let g = em.screen().row_generation(0).unwrap();
        em.advance(b"\x07");
        assert_eq!(em.bell_seq(), 4);
        assert_eq!(
            em.screen().row_generation(0).unwrap(),
            g,
            "a bell stamps no row damage",
        );
    }

    // ----- OSC 133 (FinalTerm) shell-integration marks -----

    /// A full prompt cycle marks the prompt (A) row, the output (C) row, and the command-end (D)
    /// row, and the derived [`ShellState`] + last exit status track it. B (input start) sets no
    /// row mark.
    #[test]
    fn osc_133_marks_prompt_output_and_command_end() {
        use crate::port::ShellState;
        let mut em = Emulator::new(20, 6);
        assert_eq!(
            em.screen().shell_state(),
            ShellState::Unknown,
            "no marks yet"
        );
        assert_eq!(em.screen().last_exit_status(), None);

        em.advance(b"\x1b]133;A\x07"); // A: prompt start on row 0
        em.advance(b"$ \x1b]133;B\x07"); // draw the prompt, B: input start (no mark)
        em.advance(b"ls\r\n"); // command echoed + Enter -> row 1
        assert_eq!(
            em.screen().mark(0),
            Some(PromptMark::Prompt),
            "row 0 = prompt"
        );
        assert_eq!(
            em.screen().shell_state(),
            ShellState::AtPrompt,
            "idle at the prompt / awaiting input",
        );

        em.advance(b"\x1b]133;C\x07"); // C: output starts on row 1
        assert_eq!(
            em.screen().mark(1),
            Some(PromptMark::Output),
            "row 1 = output"
        );
        assert_eq!(
            em.screen().shell_state(),
            ShellState::Running,
            "a command is running",
        );

        em.advance(b"hello\r\n"); // output -> row 2
        em.advance(b"\x1b]133;D;0\x07"); // D: finished, exit 0, row 2
        assert_eq!(
            em.screen().mark(2),
            Some(PromptMark::CommandEnd(Some(0))),
            "row 2 = command end, exit 0",
        );
        assert_eq!(
            em.screen().shell_state(),
            ShellState::AtPrompt,
            "finished -> idle again",
        );
        assert_eq!(
            em.screen().last_exit_status(),
            Some(0),
            "the reported exit status"
        );

        // A second command that FAILS (exit 1) — the newer status wins.
        em.advance(b"\x1b]133;A\x07"); // next prompt (row 3)
        em.advance(b"bad\r\n\x1b]133;C\x07\r\n\x1b]133;D;1\x07");
        assert_eq!(
            em.screen().last_exit_status(),
            Some(1),
            "the most recent command's exit wins",
        );
    }

    /// A prompt mark travels WITH its row when that row scrolls off the top into the scrollback —
    /// so a prompt in history stays a jump target and still feeds the derived state. REVERT-PROOF:
    /// if the scroll dropped the mark, `scrollback_mark(0)` would be `None`.
    #[test]
    fn osc_133_mark_scrolls_into_the_scrollback_with_its_row() {
        use crate::port::ShellState;
        let mut em = Emulator::new(8, 2); // 2 visible rows -> a quick scroll
        em.advance(b"\x1b]133;A\x07a"); // row 0: prompt mark + an 'a'
        assert_eq!(em.screen().mark(0), Some(PromptMark::Prompt));
        // Two line feeds push row 0 off the top into the scrollback.
        em.advance(b"\r\nb\r\nc");
        assert_eq!(
            em.screen().scrollback_mark(0),
            Some(PromptMark::Prompt),
            "the prompt row's mark scrolled into history with it",
        );
        assert_eq!(
            em.screen().mark(0),
            None,
            "the new visible row 0 is unmarked"
        );
        assert_eq!(
            em.screen().shell_state(),
            ShellState::AtPrompt,
            "the state still derives from the scrollback mark",
        );
    }

    /// A mark follows its LOGICAL line through a reflow: it re-attaches to the re-broken line's
    /// FIRST physical row, whether the line rejoins (widen) or re-wraps (narrow). REVERT-PROOF: if
    /// reflow dropped marks, `mark(0)` after a resize would be `None`.
    #[test]
    fn osc_133_prompt_mark_follows_its_line_through_reflow() {
        let mut em = Emulator::new(8, 4);
        em.advance(b"\x1b]133;A\x07"); // row 0 prompt
        em.advance(b"0123456789"); // wraps at width 8: row0 "01234567" (mark), row1 "89"
        assert_eq!(em.screen().mark(0), Some(PromptMark::Prompt));
        assert!(em.screen().wrapped(0));

        em.resize(16, 4); // widen: the logical line rejoins onto one row
        assert_eq!(em.screen().row_text(0), "0123456789");
        assert_eq!(
            em.screen().mark(0),
            Some(PromptMark::Prompt),
            "mark stays on the rejoined line's head",
        );

        em.resize(4, 6); // narrow: re-break to three rows
        assert_eq!(em.screen().row_text(0), "0123");
        assert_eq!(
            em.screen().mark(0),
            Some(PromptMark::Prompt),
            "mark on the re-broken line's head",
        );
        assert_eq!(em.screen().mark(1), None, "not on a continuation row");
        assert_eq!(em.screen().mark(2), None);
    }
}
