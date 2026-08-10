//! `GridBuffer` -> a real terminal, as [`Change`]s.
//!
//! This is the terminal frontend's UNIT half: the pure function that turns the cells a
//! [`HostClient`](sprag_host::HostClient) serves into the escape-sequence vocabulary a terminal
//! understands. It is the character-cell peer of `sprag-gui`'s pixel work, and it lives here for
//! the reason `sprag_client`'s crate docs give: the shared client hands out `GridBuffer`s and
//! knows nothing about the unit either frontend measures them in.
//!
//! # Why `termwiz::surface::Surface` and not hand-written ANSI
//!
//! A bespoke writer would have to be right about terminal capabilities across a dozen emulators,
//! which is exactly the class of thing a mature renderer already encodes and a fresh one gets
//! wrong on someone else's box. Emitting [`Change`]s into a [`Surface`](termwiz::surface::Surface)
//! means the diffing and the terminfo-correct output are termwiz's problem, and the work left here
//! is a pure mapping — testable with no terminal at all, which is how every test in this module
//! runs.
//!
//! # What the mapping cost, measured
//!
//! The H1 design carried a HYPOTHESIS: that pinion's cell vocabulary maps onto termwiz's losslessly.
//! Measured, it is one-to-one on every axis but two, and both exceptions are stated where they are
//! made:
//!
//! * **Intensity.** pinion carries `bold` and `dim` as independent flags; termwiz carries one
//!   [`Intensity`] axis. See [`cell_attributes`] — the combination is unreachable through sprag's
//!   own emulator, which is what makes the collapse lossless in practice rather than in principle.
//! * **Column width.** pinion's [`CellWidth`] and termwiz's own measurement of a cluster are
//!   computed by DIFFERENT unicode tables (`unicode-width` in `sprag-vt`, termwiz's `widechar_width`
//!   here), so they can disagree. [`pane_changes`] does not assume they agree — it checks, and
//!   re-anchors the cursor when they do not.
//!
//! Everything else — the six underline styles, the three colour forms, the six cursor shapes,
//! OSC 8 hyperlinks — has an exact counterpart, which is unsurprising: `sprag-vt` builds pinion's
//! cells FROM termwiz's in the first place, so this module is closing a circle rather than
//! crossing a border.

use std::sync::Arc;

use pinion_core::style::Color as PinColor;
use pinion_core::{
    CellWidth, CursorShape as PinCursorShape, GridBuffer, Hyperlink as PinHyperlink, HyperlinkId,
    TermCell, TermColor, UnderlineStyle as PinUnderlineStyle,
};
use sprag_grid::ProjectionToken;
use sprag_host::PaneAgent;
use sprag_host::chooser::Pick;
use sprag_host::keyhelp::{KeyHelp, Row, Scroll};
use sprag_host::status::Status;
use sprag_terminal::PaneId;
use sprag_terminal::tiling::{Divider, Rect};
use termwiz::cell::{Blink, CellAttributes, Intensity, Underline, unicode_column_width};
use termwiz::color::{ColorAttribute, SrgbaTuple};
use termwiz::hyperlink::Hyperlink;
use termwiz::surface::{Change, CursorShape, CursorVisibility, Position};
use unicode_segmentation::UnicodeSegmentation;

use sprag_terminal::SplitDir;

/// What a cell prints when the buffer has nothing to say about it — a blank that still occupies
/// its column, which is the rule every gap in this module follows (see [`printed`]).
const BLANK: &str = " ";

/// `grid` as terminal changes covering `area` exactly, ready for
/// [`Surface::add_changes`](termwiz::surface::Surface::add_changes).
///
/// Every cell of the rectangle is written and nothing outside it is touched, so no clear is needed
/// and none is emitted: a [`Change::ClearScreen`] would fight the surface's own diffing, repainting
/// rows that did not change — and with more than one pane on screen it would blank the others.
/// Because [`tile`](crate::tile) partitions the terminal exactly, a caller that paints every pane
/// and every divider has written every cell, which is what makes clearing unnecessary rather than
/// merely cheap.
///
/// **The rectangle is the authority, not the buffer.** A pane's grid catches up to a resize one
/// poll-wake behind the layouter, so the two disagree routinely and in both directions:
///
/// * a grid SHORTER or narrower than its rectangle leaves cells this function BLANKS, because what
///   is under them is the previous frame's — another pane's content, at the old arrangement;
/// * a grid LARGER than its rectangle is clipped, and a wide cluster that would straddle the right
///   edge is blanked rather than half-drawn, since its second column belongs to the divider.
///
/// The cursor is deliberately NOT emitted here — see [`cursor_changes`], which the caller runs last
/// and only for the pane that has focus.
///
/// # Runs, and the column check inside them
///
/// Cells are batched into runs of equal attributes — one [`Change::AllAttributes`] and one
/// [`Change::Text`] per run — because a per-cell change list would be `cols * rows` entries for a
/// screen that is mostly one style.
///
/// A run also ENDS when termwiz's measurement of the text so far stops matching the columns those
/// cells were supposed to occupy, and that check is the load-bearing part. `sprag-vt` computes
/// [`CellWidth`] with the `unicode-width` crate while termwiz measures with its own
/// `widechar_width` tables; the two are independent implementations of an evolving standard and
/// they can disagree (emoji sequences and ambiguous-width characters are where they historically
/// do). If they ever disagree here, every remaining cell of the row would shift — a whole line of
/// garbage from one character. Closing the run and re-anchoring at an absolute column confines the
/// disagreement to the single cell that caused it: the corrective write comes AFTER the wide
/// cluster that clobbered its neighbour, so the surface ends up right either way.
/// `from` is the pane's own `(col, row)` that `area`'s top-left corner shows — `(0, 0)` unless this
/// client's [`Viewport`](crate::Viewport) has scrolled past part of the pane. It is a parameter
/// rather than folded into `area` because the two are different coordinate spaces: `area` says
/// where on the TERMINAL to write, `from` says where in the PANE to read, and a single rectangle
/// carrying both would be right only while they agree.
#[must_use]
pub fn pane_changes(grid: &GridBuffer, area: Rect, from: (u16, u16)) -> Vec<Change> {
    pane_rows_changes(grid, area, from, 0..area.rows)
}

/// [`pane_changes`] for a chosen subset of the rectangle's rows, the others left to whatever the
/// surface already holds.
///
/// Row indices are the RECTANGLE's, not the screen's and not the PANE's — the same coordinates the
/// loop below counts in. The pane's own row is `from.1 + row`, which is what the caller choosing
/// them ([`PaintCache`]) has to add before indexing damage stamps: those are numbered from the
/// pane's first row, and a viewport that has scrolled past some of them makes the two differ.
///
/// Skipping a row is only ever correct against something that moves whenever the row's cells would
/// differ, and it is not this function's place to decide that: it writes what it is told and
/// nothing else. Every guarantee lives in [`PaintCache`].
#[must_use]
fn pane_rows_changes(
    grid: &GridBuffer,
    area: Rect,
    from: (u16, u16),
    rows: impl Iterator<Item = u16>,
) -> Vec<Change> {
    if area.is_empty() {
        return Vec::new();
    }

    let mut changes = Vec::new();
    // The interned hyperlink last resolved, kept so a run of linked cells builds ONE `Arc` rather
    // than one per cell — the table is interned on the producer's side for the same reason.
    let mut interned: Option<(HyperlinkId, Arc<Hyperlink>)> = None;

    for row in rows {
        // Every row is anchored absolutely rather than by trusting where the previous row's text
        // left the cursor: a row whose cells fill the last column would otherwise depend on the
        // terminal's autowrap setting, which is not this crate's to assume. With several panes on
        // one screen it is also the only correct anchor — the previous row's text ended at THIS
        // pane's right edge, not at the screen's.
        let mut run = Run::new(area.row + row, area.col);
        // Whether the previous cell was a wide HEAD, which is what makes the next cell's
        // `Trailer` its second column rather than a column of its own. Per row, because a wide
        // cluster never straddles a row boundary.
        let mut after_wide = false;
        for col in 0..area.cols {
            let follows_wide = std::mem::replace(&mut after_wide, false);
            // How many columns of the rectangle are left, which is what decides whether a wide
            // cluster can be drawn here at all.
            let room = usize::from(area.cols - col);
            let (text, columns, attrs) = match grid.cell(from.0 + col, from.1 + row) {
                Some(cell) => {
                    let Some((text, columns)) = printed(cell, follows_wide) else {
                        // A trailer behind its own head: the head's cluster already occupies this
                        // column, and its attributes are the head's by construction (pinion's
                        // `TermCell::trailer` copies them), so writing anything here would draw a
                        // glyph nobody asked for.
                        continue;
                    };
                    let link = cell
                        .hyperlink
                        .and_then(|id| grid.hyperlink(id).map(|link| (id, link)));
                    let attrs = cell_attributes(cell, resolve_link(&mut interned, link));
                    if columns > room {
                        // A wide cluster in the rectangle's last column. Its second half belongs to
                        // the divider or to the pane beyond it, so the cluster is dropped and its
                        // column blanked — the same substitution a terminal makes at its own right
                        // margin, and the alternative is a glyph bleeding into a neighbour.
                        (BLANK, 1, attrs)
                    } else {
                        after_wide = cell.width == CellWidth::Wide;
                        (text, columns, attrs)
                    }
                }
                // Inside the rectangle, outside the buffer: the pane has not caught up to its
                // size yet. Blanked rather than skipped, because whatever is under it belongs to
                // the arrangement this frame replaced.
                None => (BLANK, 1, CellAttributes::default()),
            };
            if run.attrs.as_ref() != Some(&attrs) {
                run.flush(&mut changes);
                run.restart(area.col + col, attrs);
            }
            run.text.push_str(text);
            run.span += columns;
            // The width cross-check (see the doc comment): if termwiz will not advance by the
            // columns these cells claim, the next cell must be re-anchored. The next cell is at
            // `col + columns` — one past a narrow cell, two past a wide one, whose trailer is
            // skipped.
            if unicode_column_width(&run.text, None) != run.span {
                let attrs = run.attrs.clone();
                run.flush(&mut changes);
                run.restart(
                    (area.col + col).saturating_add(u16::try_from(columns).unwrap_or(1)),
                    attrs.unwrap_or_default(),
                );
            }
        }
        run.flush(&mut changes);
    }

    changes
}

/// The prompt ROW: the question this client is asking, painted over the bottom line of `area`.
///
/// # Why it is painted OVER the panes rather than given a row of its own
///
/// This client reports its own area to the daemon, which arbitrates the WINDOW size across every
/// attached client. A row taken out of the window while a prompt is up would resize every OTHER
/// client's panes because this one asked a question, and give them all back when it closed —
/// exactly the cost [`agent_window_title`] declined a permanent status row for. So the row is an
/// OVERLAY: the arrangement underneath is untouched, and closing the prompt repaints the frame.
///
/// `caret` is the byte offset the cursor sits at within `answer` ([`Line::cursor`]), or [`None`]
/// for a question with nothing to type into. It is measured HERE, with this painter's own
/// [`unicode_column_width`] — the shared editor deliberately reports an offset rather than a column
/// count, because how wide a cluster is belongs to the surface that draws it.
///
/// [`Line::cursor`]: sprag_host::prompt::Line::cursor
#[must_use]
pub fn prompt_changes(
    area: Rect,
    question: &str,
    answer: &str,
    caret: Option<usize>,
) -> Vec<Change> {
    if area.is_empty() {
        return Vec::new();
    }
    let row = area.row + area.rows - 1;
    let width = usize::from(area.cols);
    // REVERSE VIDEO, which is what every multiplexer's status line wears and what a pane's own
    // output almost never does: the row has to read as the client speaking rather than as the
    // program underneath it, and this client has no colour scheme of its own to spend.
    let mut attrs = CellAttributes::default();
    attrs.set_reverse(true);
    let mut changes = vec![
        Change::AllAttributes(attrs),
        Change::CursorPosition {
            x: Position::Absolute(usize::from(area.col)),
            y: Position::Absolute(usize::from(row)),
        },
    ];
    // Truncated by COLUMNS, not characters, and blank-filled to the end of the row — both by
    // [`push_clipped`], which the help view shares. The tail is dropped rather than the head: the
    // question says which verb is being answered, which is the half a user cannot reconstruct.
    push_clipped(&mut changes, &format!("{question} {answer}"), width);
    if let Some(caret) = caret {
        let before = unicode_column_width(question, None)
            + 1
            + unicode_column_width(&answer[..caret.min(answer.len())], None);
        // Clamped to the row: a name longer than the terminal is wide leaves the caret at the edge
        // rather than off the screen, which is where the text it is editing has been truncated to.
        let at = usize::from(area.col) + before.min(width.saturating_sub(1));
        changes.push(Change::CursorPosition {
            x: Position::Absolute(at),
            y: Position::Absolute(usize::from(row)),
        });
        changes.push(Change::CursorVisibility(CursorVisibility::Visible));
        changes.push(Change::CursorShape(CursorShape::SteadyBar));
    } else {
        // A yes/no has nothing to edit, so the caret would only draw attention to a place the user
        // cannot type. Hidden rather than parked at the end.
        changes.push(Change::CursorVisibility(CursorVisibility::Hidden));
    }
    changes
}

/// The HELP view: what the keys do, painted over the whole of `area`.
///
/// # Why it covers the screen where the prompt covers one row
///
/// [`prompt_changes`] borrows the bottom line because a question is asked WHILE the user is looking
/// at their panes — the answer is about the thing underneath. This is the opposite: a reader here
/// has stopped working to find out what a key does, the table is thirty-odd rows in a terminal that
/// may hold twenty-four, and a view that left the panes visible would be competing with its own
/// content for the eye. It is still an OVERLAY and not a resize, for `prompt_changes`' reason
/// exactly: this client's area is arbitrated across every attached client, so taking rows would
/// reflow somebody else's panes because this user pressed `?`.
///
/// The first row is the header and the rest is the viewport, so `area.rows - 1` rows of the view
/// are shown. `scroll` is clamped by [`Scroll::offset`] against that number rather than trusted,
/// which is what makes a terminal RESIZE while the view is open safe: the offset that fitted the
/// old height cannot strand the new one past the end.
///
/// [`Scroll::offset`]: sprag_host::keyhelp::Scroll::offset
#[must_use]
pub fn help_changes(area: Rect, help: &KeyHelp, scroll: Scroll) -> Vec<Change> {
    if area.is_empty() {
        return Vec::new();
    }
    let width = usize::from(area.cols);
    let viewport = help_viewport(area);
    let offset = scroll.offset(help.len(), viewport);
    let mut changes = Vec::new();
    // REVERSE VIDEO on the header alone, which is `prompt_changes`' rule and the same argument: the
    // header is the client speaking, and the rows below it are content a user reads at length.
    let mut header_attrs = CellAttributes::default();
    header_attrs.set_reverse(true);
    // What the header says is the two things a reader needs and cannot guess: how to leave, and
    // whether there is more. The scroll marks are ASCII rather than arrows — this row is painted
    // into whatever terminal the user attached with, and a header is the wrong place to discover
    // that a font has no glyph.
    let mut header = "keys — q or Esc to close".to_owned();
    if scroll.more_above(help.len(), viewport) || scroll.more_below(help.len(), viewport) {
        header.push_str(", PgUp/PgDn to scroll");
    }
    if scroll.more_below(help.len(), viewport) {
        header.push_str(" (more below)");
    } else if scroll.more_above(help.len(), viewport) {
        header.push_str(" (end)");
    }
    changes.push(Change::AllAttributes(header_attrs));
    changes.push(Change::CursorPosition {
        x: Position::Absolute(usize::from(area.col)),
        y: Position::Absolute(usize::from(area.row)),
    });
    push_clipped(&mut changes, &header, width);
    let body_attrs = CellAttributes::default();
    for line in 0..viewport {
        changes.push(Change::AllAttributes(body_attrs.clone()));
        changes.push(Change::CursorPosition {
            x: Position::Absolute(usize::from(area.col)),
            y: Position::Absolute(usize::from(area.row) + line + 1),
        });
        // A row past the end of the view is painted as BLANKS rather than skipped: the panes are
        // still underneath, and a short table that left them showing would read as a broken frame.
        let text = help
            .rows()
            .nth(offset + line)
            .map_or_else(String::new, |row| help_row_text(row, help.chord_width()));
        push_clipped(&mut changes, &text, width);
    }
    // Nothing here is being typed into, so the caret would only mark a place the user cannot edit —
    // the same decision the yes/no prompt makes.
    changes.push(Change::CursorVisibility(CursorVisibility::Hidden));
    changes
}

/// Paint the CHOOSER over the screen — the query row, then the rows it narrows to (R315).
///
/// [`help_changes`]' twin, and the two share their shape deliberately: both take the whole screen,
/// both keep the panes underneath from showing through by painting blanks, and both put the
/// client's own line in reverse video so a reader can tell what is sprag speaking from what is
/// their own workspace. What this adds is a SELECTED row, which is the only thing on either surface
/// a keystroke moves.
///
/// Which rows exist, what they say and which one is picked are all [`Pick`]'s — this decides the
/// indent, the marker column and what happens when the list is taller than the screen.
#[must_use]
pub fn chooser_changes(area: Rect, pick: &Pick, refusal: Option<&str>) -> Vec<Change> {
    if area.is_empty() {
        return Vec::new();
    }
    let width = usize::from(area.cols);
    let viewport = help_viewport(area);
    let rows = pick.visible();
    // The window is scrolled to KEEP THE SELECTION ON SCREEN rather than to a stored offset, so a
    // filter that moves the cursor twenty rows down cannot leave a person looking at a list whose
    // picked row is somewhere else. There is no second scroll state to go stale.
    let at = pick.cursor_at().unwrap_or(0);
    let offset = at.saturating_sub(viewport.saturating_sub(1)).max(
        // ...and when the whole list fits, the top is the top.
        rows.len().saturating_sub(viewport).min(at),
    );
    let mut changes = Vec::new();
    let mut header_attrs = CellAttributes::default();
    header_attrs.set_reverse(true);
    changes.push(Change::AllAttributes(header_attrs));
    changes.push(Change::CursorPosition {
        x: Position::Absolute(usize::from(area.col)),
        y: Position::Absolute(usize::from(area.row)),
    });
    // The QUERY is the header, because it is what a keystroke edits — and it says how to leave, for
    // `help_changes`' reason: a surface that owns the keyboard has to say how to give it back.
    push_clipped(
        &mut changes,
        &match refusal {
            // Two spaces, so the refusal reads as a second clause — `prompt_changes`' rule, and the
            // same reason: this row has no colour of its own to separate them with.
            // The ERRAND, not a literal: a chooser opened to MOVE a pane and one opened to go
            // somewhere paint the same rows, and a person answering the wrong question is the
            // failure that costs them a pane. Derived from `Errand::asking`, so the two frontends
            // and the binding all say one thing (R328).
            Some(why) => format!(
                "({}) {}  {why}",
                pick.errand().asking(),
                pick.query().text()
            ),
            None => format!(
                "({}) {}   Esc to close, {} row{}",
                pick.errand().asking(),
                pick.query().text(),
                rows.len(),
                if rows.len() == 1 { "" } else { "s" },
            ),
        },
        width,
    );
    for line in 0..viewport {
        let row = rows.get(offset + line);
        let mut attrs = CellAttributes::default();
        // The SELECTION is reverse video, the one decoration this surface has that a monochrome
        // terminal still shows. A colour would be prettier and is not available: this paints into
        // whatever terminal the user attached with.
        if row.is_some_and(|row| row.target == pick.cursor()) {
            attrs.set_reverse(true);
        }
        changes.push(Change::AllAttributes(attrs));
        changes.push(Change::CursorPosition {
            x: Position::Absolute(usize::from(area.col)),
            y: Position::Absolute(usize::from(area.row) + line + 1),
        });
        push_clipped(
            &mut changes,
            &row.map_or_else(String::new, |row| chooser_row_text(row)),
            width,
        );
    }
    // The caret would mark the query, which is where typing goes — but the thing a person is
    // watching is the SELECTED ROW, and two markers on one screen is one too many. Hidden, as the
    // help view and the yes/no prompt both are.
    changes.push(Change::CursorVisibility(CursorVisibility::Hidden));
    changes
}

/// The terminal split into the rectangle the PANES fill and the row this client SPEAKS in.
///
/// One type rather than two rectangles passed side by side, because the two are derived from one
/// number and a caller holding them separately could report a size to the daemon that includes a
/// row no pane will ever be given — which is the whole class of bug a status line introduces. See
/// [`Split::of`].
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Split {
    /// What every pane's rectangle is carved out of. This is what the client REPORTS to the daemon,
    /// so the arbitrated window is the space the panes actually have.
    pub panes: Rect,
    /// The bottom row: where this client says where it is and what a key just did. Empty on a
    /// terminal with no room for one — see [`Split::of`].
    pub status: Rect,
}

impl Split {
    /// Cut a `cols` x `rows` terminal into the two.
    ///
    /// **The status row is given up when there is no room**, and "no room" is one row: a terminal
    /// one row tall would otherwise leave the panes nothing at all, which trades a client that
    /// cannot show a message for a client that cannot show a SESSION. The status rectangle is empty
    /// then, and every painter here already returns nothing for an empty area — so the degradation
    /// costs no branch at any call site.
    #[must_use]
    pub const fn of(cols: u16, rows: u16) -> Self {
        if rows <= 1 {
            return Self {
                panes: Rect::screen(cols, rows),
                status: Rect::new(0, 0, 0, 0),
            };
        }
        Self {
            panes: Rect::screen(cols, rows - 1),
            status: Rect::new(0, rows - 1, cols, 1),
        }
    }

    /// The whole terminal this was cut from — what a client can DRAW on, as opposed to what it
    /// gives the panes.
    ///
    /// Kept as a method rather than as a third field so the two cannot disagree, and it earns its
    /// place: a live test caught a surface sized to [`panes`](Self::panes) instead, which CLAMPS
    /// the status row's absolute cursor move, so the row painted one line high and the terminal's
    /// real bottom row was never written at all.
    #[must_use]
    pub const fn terminal(&self) -> Rect {
        Rect::screen(self.panes.cols, self.panes.rows + self.status.rows)
    }
}

/// The status row: where this client is, or what a key just did.
///
/// **A message REPLACES the line rather than sharing it**, which is tmux's own behaviour and is
/// the honest reading of a single row: half a location beside half a refusal is two truncated
/// facts. The location comes back when the message expires, and nothing has to remember it —
/// [`Status`] is derived from the host on every paint.
///
/// `view` is what this client is SHOWING of the window it is watching
/// ([`Viewport::note`](crate::Viewport::note)) — `None` for the ordinary client that can see all of
/// it, and so absent from every row this front painted before a viewport existed.
#[must_use]
pub fn status_changes(
    area: Rect,
    status: &Status,
    message: Option<&str>,
    view: Option<&str>,
) -> Vec<Change> {
    if area.is_empty() {
        return Vec::new();
    }
    let mut attrs = CellAttributes::default();
    // Reverse video, like the chooser's header and the key table's — the one decoration available
    // in every terminal this client can be attached from, and what makes the row read as CHROME
    // rather than as a pane's last line of output.
    attrs.set_reverse(true);
    let mut changes = vec![
        Change::AllAttributes(attrs),
        Change::CursorPosition {
            x: Position::Absolute(usize::from(area.col)),
            y: Position::Absolute(usize::from(area.row)),
        },
    ];
    // **THE LAST CELL OF THE BOTTOM ROW IS NEVER WRITTEN**, and that is not a cosmetic margin: a
    // character placed in the bottom-right corner leaves the terminal with a pending wrap, and the
    // next thing written SCROLLS the screen — taking the top pane row with it and moving this row
    // out from under the client that just drew it. A live test caught exactly that (the whole
    // screen blank with one status line stranded a row too high, after a pane produced enough
    // output to force repeated repaints), which is why the bound is here rather than in a comment.
    //
    // Nothing else paints that cell — the status row is outside the tiling by construction
    // ([`Split`]) — so what it holds is whatever the last `Clear` left, which is a blank.
    // The viewport's note LEADS the line, and a MESSAGE replaces both — the row is one sentence at a
    // time, and a message is the newer one. Leading rather than trailing because the client that
    // owes this note is by definition the narrow one, so a note after the session and its windows
    // is the note truncated away on exactly the terminal it exists for.
    let line = message.map_or_else(
        || match view {
            Some(view) => format!("{view} {}", status.line()),
            None => status.line(),
        },
        str::to_owned,
    );
    push_clipped(
        &mut changes,
        &line,
        usize::from(area.cols.saturating_sub(1)),
    );
    changes
}

/// One chooser [`Row`](sprag_host::chooser::Row) as this surface lays it out.
///
/// The indent is TWO SPACES per level and the marker is `*`, both this surface's decisions: a depth
/// is a number in the shared type precisely so a terminal that cannot draw box characters and a GUI
/// that can are not forced to agree. The marker names where the client already IS, which is the one
/// row a person is orienting from.
fn chooser_row_text(row: &sprag_host::chooser::Row) -> String {
    format!(
        "{}{} {}  {}",
        "  ".repeat(usize::from(row.depth)),
        if row.here { "*" } else { " " },
        row.label,
        row.detail,
    )
}

/// How many rows of the help view fit on `area` — the screen less its header row.
///
/// One function so the painter and whatever handles the keys cannot disagree about the size of a
/// page. It is the painter's own arithmetic, named and exported rather than repeated in the client,
/// because a page-down that moved by a different number than the screen shows is a reader losing
/// lines between two pages — the failure [`Scroll::page`]'s one-row overlap exists to prevent.
///
/// [`Scroll::page`]: sprag_host::keyhelp::Scroll::page
#[must_use]
pub fn help_viewport(area: Rect) -> usize {
    usize::from(area.rows).saturating_sub(1)
}

/// One [`Row`] as this surface lays it out — the chord column, then what it does.
///
/// The TEXT of every row is the shared module's ([`Row`]'s own `Display`); what this adds is the
/// COLUMN, which is a layout decision and so belongs to the surface. A heading is not indented and
/// a binding is, so the groups read as groups without a box being drawn round them.
fn help_row_text(row: &Row, chord_width: usize) -> String {
    match row {
        Row::Heading(text) => text.clone(),
        Row::Blank => String::new(),
        Row::Bind {
            chord,
            action,
            repeat,
        } => {
            // Padded in CHARACTERS, and the pad is computed rather than given to `{:width$}`,
            // because a chord may hold a multi-byte key a user bound and the formatter counts
            // bytes. `sprag list-keys` measures its own column the same way and for the same reason.
            let pad = " ".repeat(chord_width.saturating_sub(chord.chars().count()));
            // The MARK is the shared module's two characters ([`KeyHelp::REPEAT`]); the column that
            // keeps the actions lined up whether or not a row has one is this surface's.
            let mark = if *repeat { KeyHelp::REPEAT } else { "  " };
            format!("  {chord}{pad}  {mark} {action}")
        }
        Row::Vocabulary { form, bound } => {
            if *bound {
                format!("  {form}")
            } else {
                format!("  {form}  ({})", KeyHelp::UNBOUND)
            }
        }
    }
}

/// Write `text` at the cursor, cut to `width` COLUMNS and blank-filled to it.
///
/// Columns rather than characters for [`prompt_changes`]' reason — a name in CJK fills two cells per
/// glyph — and the fill is what makes an overlay opaque: without it a pane's output shows through
/// the short rows of the thing drawn over it.
fn push_clipped(changes: &mut Vec<Change>, text: &str, width: usize) {
    let mut painted = String::new();
    let mut columns = 0;
    for cluster in text.graphemes(true) {
        let cluster_width = unicode_column_width(cluster, None);
        if columns + cluster_width > width {
            break;
        }
        painted.push_str(cluster);
        columns += cluster_width;
    }
    changes.push(Change::Text(painted));
    if columns < width {
        changes.push(Change::Text(" ".repeat(width - columns)));
    }
}

/// The glyphs of one divider — the line of cells between two panes.
///
/// A `Horizontal` split lays its panes side by side, so what separates them is a VERTICAL line, and
/// a `Vertical` split's is horizontal. The vocabulary is the host's and tmux's (`-h` names the
/// layout, not the line), so the inversion is stated here once rather than at each call site.
///
/// Junctions are deliberately not drawn: where a divider meets another at a T, both cells keep
/// their own straight glyph rather than becoming a box-drawing tee. tmux draws the tee; the line
/// reads correctly without it, and inferring a junction means asking what the NEIGHBOURING cells
/// hold, which is a second pass over a partition this function is handed one piece of.
#[must_use]
pub fn divider_changes(divider: &Divider) -> Vec<Change> {
    if divider.area.is_empty() {
        return Vec::new();
    }
    let glyph = match divider.dir {
        SplitDir::Horizontal => "\u{2502}",
        SplitDir::Vertical => "\u{2500}",
    };
    // Its own attributes, not whatever the last pane's run left set: a divider inheriting a
    // program's reverse-video would read as a selection.
    let mut changes = vec![Change::AllAttributes(CellAttributes::default())];
    for row in 0..divider.area.rows {
        changes.push(Change::CursorPosition {
            x: Position::Absolute(usize::from(divider.area.col)),
            y: Position::Absolute(usize::from(divider.area.row + row)),
        });
        changes.push(Change::Text(glyph.repeat(usize::from(divider.area.cols))));
    }
    changes
}

/// The OUTER terminal's window title for a client attached to `session`, reporting what each pane's
/// agent is doing (H3 slice 5).
///
/// # Why the title is this client's agent surface at all
///
/// This is the frontend with NO chrome. It paints panes, the lines between them, and one cursor —
/// deliberately, and its own paint docs give the reason ("what makes the focused pane identifiable
/// without a coloured border"). So it has no pane list to hang a marker on, and the two ways to make
/// one are not equal:
///
/// * A STATUS ROW would have to come out of the window, which means this client reporting a smaller
///   area to the daemon, which re-arbitrates `window-size` and reflows every pane in the session — and
///   it would do that each time an agent starts or exits. A row is also a permanent piece of chrome
///   whose contents are a front of their own (tmux's `status-left` / `status-right` / formats).
/// * The outer terminal's TITLE costs no row, no reflow and no arbitration, and it is where this
///   project ALREADY puts a pane's state for the other frontend: `sprag-gui` writes the focused pane's
///   display title — the very string carrying the agent marker — into its OS window title. A client
///   owning the title of the terminal it was launched in is also what tmux does (`set-titles`).
///
/// The title is visible when the window is NOT focused, which for the state this front exists for
/// ("come back to me") is the more useful half: a user working elsewhere sees the tab change.
///
/// # The shape, and what each part of it is for
///
/// `sprag: work` with nothing to report, and `sprag: work — claude needs an answer (pane 3), claude
/// working (pane 1)` with agents. Ordered by [`sprag_client::agent_urgency`] and NOT by pane id,
/// because a terminal truncates a title from the right: the pane a person has to go to must be in the
/// part that survives. Ties keep pane-id order, so a title does not reshuffle between two equally
/// urgent panes on every wake.
///
/// The pane ID is named because it is the only handle this client's panes HAVE — it shows no numbers
/// anywhere — and it is the same id `sprag panes`, `sprag agent` and the MCP tools all take, so a
/// title tells a user what to type. It trails the phrase rather than leading it for the truncation
/// reason again: the state is what a glance needs.
///
/// A pane the wire says nothing about contributes nothing (D8's additive rule at this surface), and a
/// workspace of shells therefore produces exactly the baseline — which is what makes the digest's
/// appearance itself meaningful.
#[must_use]
pub fn agent_window_title(session: &str, agents: &[(PaneId, PaneAgent)]) -> String {
    let baseline = format!("sprag: {session}");
    if agents.is_empty() {
        return baseline;
    }
    let mut ordered: Vec<&(PaneId, PaneAgent)> = agents.iter().collect();
    // A STABLE sort, so equal urgencies keep the pane order the caller passed (host order) rather
    // than swapping under the user between two wakes that say the same thing.
    ordered.sort_by_key(|(_, agent)| sprag_client::agent_urgency(&agent.state));
    let digest = ordered
        .iter()
        .map(|(id, agent)| format!("{} (pane {id})", sprag_client::agent_phrase(agent)))
        .collect::<Vec<_>>()
        .join(", ");
    format!("{baseline} \u{2014} {digest}")
}

/// The [`Change`] that puts `wanted` on the terminal's title bar — or `None` when the terminal is
/// already showing it, advancing `held` to whatever was decided.
///
/// The skip is not an optimisation, and pulling it out here is what lets a test say so.
/// [`Surface::add_change`](termwiz::surface::Surface::add_change) RECORDS every change it is handed and
/// the flush renders them, so a title handed over on every frame is one OSC per repaint — that is once
/// per keystroke, on the path R246 measured this client's whole cost model on. A title is also the one
/// thing here a terminal may show somewhere persistent (a tab, a window list), so rewriting it
/// needlessly is visible work rather than merely wasted work.
///
/// `held` is the client's own record of what it last SET, not a read of the terminal: there is no way
/// to ask a terminal what its title is, so the only honest baseline is what was sent. `None` therefore
/// means "we have set none yet", which is why the first call always answers `Some`.
pub fn title_change(held: &mut Option<String>, wanted: String) -> Option<Change> {
    if held.as_deref() == Some(wanted.as_str()) {
        return None;
    }
    *held = Some(wanted.clone());
    Some(Change::Title(wanted))
}

/// What a cell prints and how many columns that print is supposed to occupy, or `None` when the
/// cell prints nothing because a preceding wide cluster already covers it.
///
/// The column count is what the width cross-check in [`pane_changes`] compares termwiz's own
/// measurement against, so it must be the count pinion INTENDED, never a measurement of the text:
/// a wide head claims BOTH of its columns here, which is why its trailer claims none.
///
/// # The orphan trailer
///
/// A `Trailer` that follows no head is a malformed buffer — nothing sprag's projection produces,
/// but constructible (`GridBuffer`'s row builder and its wire `TryFrom` both validate cell COUNTS,
/// not width coherence). It is rendered as the blank column it occupies rather than skipped,
/// because skipping it would shift every remaining cell of the row one column left: the failure
/// this whole accounting exists to prevent, arriving through the one door left open.
///
/// A cell with an empty cluster is blanked for the same reason — it occupies its column either
/// way, and printing nothing there moves the rest of the row.
fn printed(cell: &TermCell, follows_wide: bool) -> Option<(&str, usize)> {
    let cluster = if cell.cluster.is_empty() {
        " "
    } else {
        &cell.cluster
    };
    match cell.width {
        CellWidth::Trailer if follows_wide => None,
        CellWidth::Trailer => Some((" ", 1)),
        CellWidth::Wide => Some((cluster, 2)),
        CellWidth::Narrow => Some((cluster, 1)),
    }
}

/// One batch of same-attribute cells: where it starts, what it will print, and how many columns
/// that print is supposed to occupy.
///
/// The `span` is what makes the width cross-check possible at all — without it there is nothing to
/// compare termwiz's measurement against, and a disagreement would be discovered by the user
/// looking at a broken screen.
struct Run {
    /// The row every cell of this run is on (runs never span rows).
    row: u16,
    /// The column the run's text starts at, emitted as an absolute cursor position.
    col: u16,
    /// The attributes every cell in the run shares. `None` before the first cell, which is what
    /// makes an empty run flush to nothing.
    attrs: Option<CellAttributes>,
    /// The clusters, concatenated.
    text: String,
    /// How many columns `text` is SUPPOSED to occupy — the sum of the cells' declared widths, not
    /// a measurement of the string.
    span: usize,
}

impl Run {
    /// An empty run at `(col, row)`, carrying no attributes yet.
    fn new(row: u16, col: u16) -> Self {
        Self {
            row,
            col,
            attrs: None,
            text: String::new(),
            span: 0,
        }
    }

    /// Emit this run, if it has anything to say. An absolute cursor position precedes every run
    /// because a run may begin anywhere: after an attribute change, after a width disagreement, or
    /// at the start of a row.
    fn flush(&self, changes: &mut Vec<Change>) {
        let Some(attrs) = self.attrs.as_ref() else {
            return;
        };
        if self.text.is_empty() {
            return;
        }
        changes.push(Change::CursorPosition {
            x: Position::Absolute(usize::from(self.col)),
            y: Position::Absolute(usize::from(self.row)),
        });
        changes.push(Change::AllAttributes(attrs.clone()));
        changes.push(Change::Text(self.text.clone()));
    }

    /// Begin a fresh run at `col` with `attrs`, keeping the row.
    fn restart(&mut self, col: u16, attrs: CellAttributes) {
        self.col = col;
        self.attrs = Some(attrs);
        self.text.clear();
        self.span = 0;
    }
}

/// Resolve an interned link to a shared termwiz one, reusing the previous `Arc` when the id has
/// not moved.
///
/// A run of linked cells all name the same [`HyperlinkId`] — that is what interning is for — so
/// building a fresh `Arc<Hyperlink>` per cell would allocate once per character of a hyperlinked
/// path for no gain: the attributes compare equal either way, so the run would not even split.
fn resolve_link<'a>(
    interned: &'a mut Option<(HyperlinkId, Arc<Hyperlink>)>,
    link: Option<(HyperlinkId, &PinHyperlink)>,
) -> Option<&'a Arc<Hyperlink>> {
    let (id, link) = link?;
    if interned.as_ref().is_none_or(|(seen, _)| *seen != id) {
        let built = match link.id.as_deref() {
            // The OSC-8 `id=` param is the GROUPING key: two runs sharing a non-empty id are one
            // logical link. It is carried across rather than dropped, so a terminal that groups
            // (a soft-wrapped path highlighting as one link) still can.
            Some(group) => Hyperlink::new_with_id(link.uri.clone(), group.to_owned()),
            None => Hyperlink::new(link.uri.clone()),
        };
        *interned = Some((id, Arc::new(built)));
    }
    interned.as_ref().map(|(_, link)| link)
}

/// One pinion cell's style as termwiz attributes.
///
/// # The one lossy axis, and why it is not lossy here
///
/// pinion models SGR 1 (bold) and SGR 2 (dim) as INDEPENDENT booleans; termwiz models them as one
/// [`Intensity`] with three values, so `bold && dim` has no representation. That combination is
/// **unreachable in the buffers this crate paints**: sprag's own emulator makes the two mutually
/// exclusive at the SGR — `Sgr::Intensity(Bold)` clears `dim` and `Sgr::Intensity(Half)` clears
/// `bold` (`sprag_vt`'s emulator) — which is also what termwiz's own parser does, because it is the
/// same one-axis model. So the collapse loses nothing that can arrive.
///
/// It is still given a deterministic rule, because the function must be total over a type that
/// admits the pair: **bold wins**. The rule exists so a hand-built buffer cannot produce
/// unpredictable output, not because the case occurs.
///
/// Every other axis is exact: the six underline styles are the same six, blink folds to
/// [`Blink::Slow`] (pinion folds SGR 5 and 6 into one flag on the way in, so there is no rapid
/// blink to lose), `hidden` is termwiz's `invisible`, and the three colour forms map through this
/// module's `term_color` — resolved by the projection before they arrive, so what a host sends is
/// always the truecolor arm.
#[must_use]
pub fn cell_attributes(cell: &TermCell, link: Option<&Arc<Hyperlink>>) -> CellAttributes {
    let mut attrs = CellAttributes::default();
    attrs
        .set_foreground(term_color(cell.fg))
        .set_background(term_color(cell.bg))
        .set_intensity(if cell.attrs.bold {
            Intensity::Bold
        } else if cell.attrs.dim {
            Intensity::Half
        } else {
            Intensity::Normal
        })
        .set_underline(underline(cell.attrs.underline))
        .set_italic(cell.attrs.italic)
        .set_blink(if cell.attrs.blink {
            Blink::Slow
        } else {
            Blink::None
        })
        .set_reverse(cell.attrs.reverse)
        .set_invisible(cell.attrs.hidden)
        .set_strikethrough(cell.attrs.strikethrough)
        .set_underline_color(
            cell.underline_color
                .map_or(ColorAttribute::Default, term_color),
        )
        .set_hyperlink(link.cloned());
    attrs
}

/// A pinion terminal colour as a termwiz one — the three closed forms, each with an exact peer.
///
/// **Note what actually arrives.** `sprag-grid`'s projection resolves every colour against the
/// pane's live palette, so a `GridBuffer` from a host carries [`TermColor::Rgb`] and nothing else:
/// an OSC 4 palette redefinition restains cells at the PRODUCER, which is what lets a theme change
/// recolour a screen that was printed before it. The other two arms are therefore not dead code
/// but not the live path either — they are what a hand-built buffer (`GridBuffer::new` blanks
/// carry [`TermColor::Default`]) needs to render correctly.
///
/// One consequence worth stating because it is a real behavioural fact rather than a detail: a
/// terminal client paints the colours the HOST resolved, so a remote session does not re-theme
/// itself against the local terminal's palette. That is the same thing `sprag-gui` does, and it is
/// the projection's design (the producer owns authoritative state), not an omission here.
fn term_color(color: TermColor) -> ColorAttribute {
    match color {
        TermColor::Default => ColorAttribute::Default,
        TermColor::Indexed(index) => ColorAttribute::PaletteIndex(index),
        TermColor::Rgb(PinColor { r, g, b, a }) => {
            ColorAttribute::TrueColorWithDefaultFallback(SrgbaTuple::from((r, g, b, a)))
        }
    }
}

/// A pinion underline style as termwiz's — the same six, in the same order, because both are the
/// SGR 4:x vocabulary.
fn underline(style: PinUnderlineStyle) -> Underline {
    match style {
        PinUnderlineStyle::None => Underline::None,
        PinUnderlineStyle::Single => Underline::Single,
        PinUnderlineStyle::Double => Underline::Double,
        PinUnderlineStyle::Curly => Underline::Curly,
        PinUnderlineStyle::Dotted => Underline::Dotted,
        PinUnderlineStyle::Dashed => Underline::Dashed,
    }
}

/// The cursor of the pane occupying `area`: colour, shape, visibility, then position.
///
/// **Only the pane with FOCUS may call this, and it must be the last thing painted.** A terminal
/// has one cursor, so every pane emitting its own would leave whichever painted last in charge,
/// which is not the same thing as whichever the user is typing into. And [`Change::Text`] moves the
/// surface's cursor as it writes, so a cursor emitted before another pane's cells ends up trailing
/// that pane's last run. Both failures look like a cursor in the wrong place; only the second is
/// intermittent.
///
/// The other panes therefore show no cursor at all, which is also what tmux does and what makes the
/// focused pane identifiable without a border colour.
///
/// pinion splits the cursor's SHAPE from its BLINK mode (`shape` + `blink`); termwiz folds the two
/// into one seven-variant enum. Three shapes times two modes is exactly six of those variants, so
/// the fold is a bijection — the seventh, `Default`, is the "whatever the terminal prefers" value
/// no producer-reported cursor means.
#[must_use]
pub fn cursor_changes(grid: &GridBuffer, area: Rect, from: (u16, u16)) -> Vec<Change> {
    let cursor = grid.cursor();
    // Where the cursor is on THIS terminal: the pane's own cell, less what the viewport scrolled
    // past. A cursor above or left of the view has no screen cell at all, which `checked_sub`
    // reports as `None` and the visibility test below reads as "not here" — the same answer it
    // already gives for a cursor outside the rectangle, and for the same reason.
    let at = (
        cursor.col.checked_sub(from.0),
        cursor.row.checked_sub(from.1),
    );
    // Outside the buffer the cursor is not a position this client can honour: pinion's `GridCursor`
    // docs say the producer's position may briefly fall outside during an in-flight resize, and a
    // clamped cursor would draw an authoritative-looking block in a cell the producer never named.
    // Outside the RECTANGLE it is a position belonging to another pane, which is worse — so both
    // are reported hidden, which is the truthful rendering of "not here".
    let visible = cursor.visible
        && cursor.col < grid.cols()
        && cursor.row < grid.rows()
        && at.0.is_some_and(|col| col < area.cols)
        && at.1.is_some_and(|row| row < area.rows);
    let mut changes = vec![
        Change::CursorColor(
            cursor
                .cursor_color
                .map_or(ColorAttribute::Default, |color| {
                    term_color(TermColor::Rgb(color))
                }),
        ),
        Change::CursorShape(match (cursor.shape, cursor.blink) {
            (PinCursorShape::Block, true) => CursorShape::BlinkingBlock,
            (PinCursorShape::Block, false) => CursorShape::SteadyBlock,
            (PinCursorShape::Bar, true) => CursorShape::BlinkingBar,
            (PinCursorShape::Bar, false) => CursorShape::SteadyBar,
            (PinCursorShape::Underline, true) => CursorShape::BlinkingUnderline,
            (PinCursorShape::Underline, false) => CursorShape::SteadyUnderline,
        }),
        Change::CursorVisibility(if visible {
            CursorVisibility::Visible
        } else {
            CursorVisibility::Hidden
        }),
    ];
    if let (true, (Some(col), Some(row))) = (visible, at) {
        changes.push(Change::CursorPosition {
            x: Position::Absolute(usize::from(area.col + col)),
            y: Position::Absolute(usize::from(area.row + row)),
        });
    }
    changes
}

/// Where a pane's rectangle READS its cells, and which of the pane's own content that is.
///
/// One field with two arms rather than a pair of coordinates, because the two facts stopped being
/// the same thing when a client could re-wrap. A pane drawn directly reads the buffer at the cell
/// the viewport scrolled to, and that cell IS the content's identity. A re-wrapped pane is handed a
/// buffer already cut to its rectangle ([`sprag_grid::rewrap`]), so it reads from the origin — but
/// its identity is the re-wrapped row that buffer STARTS at, and a cache keyed on the read offset
/// would see `(0, 0)` for every frame and skip a pane whose content had scrolled underneath it.
///
/// Made a type rather than two fields so that "which do I use here" is answered by
/// [`read_at`](Self::read_at) once, instead of at every call site.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PaneSource {
    /// The pane's own buffer, drawn from this cell of it — `(0, 0)` whenever the window fits.
    Direct((u16, u16)),
    /// A buffer already cut for this client, whose first row is re-wrapped row `top` of the pane.
    Rewrapped {
        /// Which re-wrapped row the buffer's first row is. Not an index into it.
        top: u16,
    },
}

impl PaneSource {
    /// The cell of the buffer the rectangle's top-left corner reads.
    ///
    /// The ONE place the difference between the two arms is spent. A re-wrapped buffer is the
    /// window, so it is read from its origin however far the content has scrolled.
    #[must_use]
    pub const fn read_at(self) -> (u16, u16) {
        match self {
            Self::Direct(at) => at,
            Self::Rewrapped { .. } => (0, 0),
        }
    }
}

/// One pane's contribution to a frame: where it goes, what it holds, and the stamp that says how
/// much of it can have changed.
///
/// The cells and the token are taken TOGETHER
/// ([`HostClient::pane_frame`](sprag_host::HostClient::pane_frame)) and travel together from there
/// on, because a token that does not describe the buffer beside it is worse than no token at all —
/// it licenses a skip of rows the client never received.
pub struct PanePaint {
    /// Which pane.
    pub pane: PaneId,
    /// Its rectangle on THIS terminal — already cut to what this client's
    /// [`Viewport`](crate::Viewport) shows, so it is what will actually be written.
    pub area: Rect,
    /// Where its cells are read from, and what identifies them — see [`PaneSource`].
    pub source: PaneSource,
    /// The cells to write.
    pub cells: GridBuffer,
    /// The projection token those cells arrived under, or [`None`] for "cannot say".
    pub token: Option<ProjectionToken>,
}

/// What was last written to the terminal's surface, so a frame writes only the rows that can differ
/// from it.
///
/// # Why this exists
///
/// Building a pane's change list is `O(cells)` and it was being done in full for every frame. On
/// the input path that is once per KEYSTROKE — measured at 1.14 ms per repaint for an 80x24 pane
/// and **17.9 ms for 240x64**, which is more than a 60 Hz frame to put one echoed character on
/// screen. The bytes that actually reach the terminal were never the cost: `Surface`'s own diff
/// already reduces them to almost nothing (4.2 ms of flushing against 448 ms of building, over the
/// same burst). So the work to remove is the BUILDING, and the only safe way to skip it is a token
/// that moves whenever the answer would.
///
/// # What makes a skip safe
///
/// [`ProjectionToken`]'s `row_generations` are the producer's own per-row damage stamps, and the
/// invariant that an unchanged stamp means unchanged cells is not one this cache introduces: it is
/// the one pinion's `TextGrid` already rests on, and the reason `sprag-vt` stamps EVERY row on a
/// palette change rather than only the cells it re-colours. This is a second consumer of an
/// existing guarantee, not a new guarantee.
///
/// Everything the stamps do NOT cover invalidates the pane WHOLE, and each is a field the token
/// carries for exactly that reason:
///
/// * **the alternate screen** — a switch replaces the content while both screens keep their own
///   stamp counters, so equal stamps across a switch would mean nothing;
/// * **the column count** — a resize COPIES surviving rows' stamps (documented in
///   [`ProjectionToken`]), so a width change is invisible to them;
/// * **the row count** — the same argument on the other axis.
///
/// And what the token says nothing about at all is the SURFACE: the cache describes a screen, so
/// any change to which pane owns which rectangle discards it entirely ([`Self::changes`] compares
/// the whole arrangement), as does a caller that blanked the surface ([`Self::forget`]).
///
/// A pane whose host answers no token is never remembered, so it is rebuilt every frame — the
/// behaviour of every client before this existed.
#[derive(Default)]
pub struct PaintCache {
    /// Which pane owned which rectangle, showing which of its own cells, when the remembered rows
    /// were written. Compared WHOLE rather than per pane: a pane's own rectangle being unchanged
    /// does not say another pane has not written over it, and the cheapest way to never have to
    /// make that argument is not to rely on it.
    ///
    /// **The [`PaneSource`] is part of the key and not decoration.** A viewport that scrolls by a
    /// row leaves every rectangle exactly where it was and changes what each of them SHOWS; without
    /// it here, the cache would compare a pane's unmoved stamps against an unmoved rectangle and
    /// skip every row of a frame that had scrolled. It is a TYPE rather than a coordinate pair
    /// because a re-wrapped pane's identity is not the cell it reads from — see [`PaneSource`].
    arrangement: Vec<(PaneId, Rect, PaneSource)>,
    /// The token each pane's surface rows were built from. Absent for a pane whose host could not
    /// say, which is what makes "cannot say" rebuild rather than skip.
    tokens: std::collections::HashMap<PaneId, ProjectionToken>,
}

impl PaintCache {
    /// Forget everything: whatever is on the surface is no longer known.
    ///
    /// The caller's obligation on any [`Change::ClearScreen`] — a blanked surface holds none of
    /// the rows this cache would otherwise let a frame skip.
    ///
    /// The token clear is NOT independently falsifiable and is kept anyway, which is worth saying
    /// rather than leaving for a reader to discover: clearing the arrangement alone already forces
    /// the next frame whole, because any frame with a pane in it has an arrangement the empty one
    /// cannot equal. That argument holds only while no caller asks for an empty frame, and this is
    /// a public type — so `forget` clears what its name says it clears, and does not rest on a
    /// property of its callers.
    pub fn forget(&mut self) {
        self.arrangement.clear();
        self.tokens.clear();
    }

    /// The changes `panes` still owe the surface, in the order they were given.
    ///
    /// One call for the WHOLE frame rather than one per pane, so the arrangement check cannot be
    /// forgotten by a caller that painted the panes in a loop.
    #[must_use]
    pub fn changes(&mut self, panes: &[PanePaint]) -> Vec<Change> {
        let arrangement: Vec<(PaneId, Rect, PaneSource)> = panes
            .iter()
            .map(|drawn| (drawn.pane, drawn.area, drawn.source))
            .collect();
        if arrangement != self.arrangement {
            self.tokens.clear();
            self.arrangement = arrangement;
        }

        let mut changes = Vec::new();
        for drawn in panes {
            let reusable = drawn
                .token
                .as_ref()
                .zip(self.tokens.get(&drawn.pane))
                .filter(|(now, then)| comparable(now, then));
            match reusable {
                Some((now, then)) => {
                    // In the PANE's rows, which is what the stamps are numbered in: a rectangle row
                    // is the pane's `from.1 + row`, so a rectangle whose last row is past the last
                    // stamp starts being unvouchable that many rows earlier.
                    let read_at = drawn.source.read_at();
                    let stamped = drawn
                        .area
                        .rows
                        .min(row_count(now).saturating_sub(read_at.1));
                    changes.extend(pane_rows_changes(
                        &drawn.cells,
                        drawn.area,
                        read_at,
                        (0..drawn.area.rows).filter(|row| {
                            // Past the stamps is past what this cache can vouch for, so those rows
                            // are always rebuilt. In practice there are none: they are the tail of
                            // a rectangle taller than the grid, which is a pane still catching up
                            // to a resize — and a resize has already discarded the arrangement.
                            let pane_row = usize::from(read_at.1 + *row);
                            *row >= stamped
                                || now.row_generations[pane_row] != then.row_generations[pane_row]
                        }),
                    ));
                }
                None => changes.extend(pane_changes(
                    &drawn.cells,
                    drawn.area,
                    drawn.source.read_at(),
                )),
            }
            match &drawn.token {
                Some(token) => self.tokens.insert(drawn.pane, token.clone()),
                // A pane the host cannot vouch for must not leave a token behind that a later
                // frame would compare against.
                None => self.tokens.remove(&drawn.pane),
            };
        }
        changes
    }
}

/// Whether two tokens differ ONLY in ways their `row_generations` can account for.
///
/// Everything else about a projection that can change without stamping a row is checked here
/// instead, and the CURSOR is deliberately not among them: it moves on nearly every keystroke and
/// is painted by [`cursor_changes`] from the same grid, so treating it as a whole-pane
/// invalidation would rebuild every frame and save nothing.
fn comparable(now: &ProjectionToken, then: &ProjectionToken) -> bool {
    now.screen == then.screen
        && now.cols == then.cols
        && now.row_generations.len() == then.row_generations.len()
}

/// A token's row count, clamped into the row unit a rectangle counts in.
fn row_count(token: &ProjectionToken) -> u16 {
    u16::try_from(token.row_generations.len()).unwrap_or(u16::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;
    use pinion_core::{CellAttrs, GridCursor};
    use termwiz::surface::Surface;

    /// One pane's verdict, as the client reads it off the wire.
    fn agent(state: &str, name: &str) -> PaneAgent {
        PaneAgent {
            state: state.to_owned(),
            name: Some(name.to_owned()),
            rule: Some("dialog-choice-list".to_owned()),
            seq: 2,
        }
    }

    /// A workspace with no agents produces the BASELINE title and nothing else — the additive rule
    /// (D8) at this surface, which is what makes the digest's appearance mean something.
    ///
    /// REVERT-PROOF: drop the empty-case early return and the title ends in a dangling separator, so
    /// every shell-only session claims to be reporting something.
    #[test]
    fn a_workspace_of_shells_titles_the_terminal_with_the_session_alone() {
        assert_eq!(agent_window_title("work", &[]), "sprag: work");
    }

    /// The pane a person has to go to comes FIRST, whatever order the host listed the panes in.
    ///
    /// This is the assertion the ordering exists for, and it is about truncation rather than taste: a
    /// terminal cuts a title off on the right, so a blocked pane behind two working ones is a blocked
    /// pane the user never sees. D3 names `Blocked` as the state the whole front exists for.
    ///
    /// REVERT-PROOF: drop the `sort_by_key` and the blocked pane appears third — the title still
    /// mentions it, in the half a terminal does not show.
    #[test]
    fn the_title_leads_with_the_pane_that_needs_an_answer() {
        let panes = [
            (PaneId(4), agent("working", "claude")),
            (PaneId(7), agent("working", "codex")),
            (PaneId(9), agent("blocked", "claude")),
        ];
        assert_eq!(
            agent_window_title("work", &panes),
            "sprag: work \u{2014} claude needs an answer (pane 9), claude working (pane 4), \
             codex working (pane 7)",
            "blocked first; the two equal-urgency panes keep the host's order",
        );
    }

    /// A title is SET once and then only when it has changed — the equality skip
    /// ([`title_change`]), asserted rather than asserted-in-a-comment.
    ///
    /// The cost it exists to avoid is one OSC per repaint, which on this client is one per keystroke.
    /// Nothing about the screen would look wrong, which is precisely why it needs a test: a mechanism
    /// whose absence is invisible is one that gets deleted by the next tidy-up.
    ///
    /// REVERT-PROOF: return `Some` unconditionally and the second assertion fails; never advance
    /// `held` and the same one does (every frame would re-send).
    #[test]
    fn the_terminals_title_is_set_once_and_then_only_when_it_moves() {
        let mut held = None;
        let first = title_change(&mut held, "sprag: work".to_owned());
        assert!(
            matches!(first, Some(Change::Title(ref t)) if t == "sprag: work"),
            "the first title is always sent — a client cannot ask a terminal what it is showing: \
             {first:?}",
        );
        assert!(
            title_change(&mut held, "sprag: work".to_owned()).is_none(),
            "an unchanged digest costs nothing at all",
        );
        let moved = title_change(
            &mut held,
            "sprag: work \u{2014} claude idle (pane 1)".to_owned(),
        );
        assert!(
            matches!(moved, Some(Change::Title(_))),
            "and a state that MOVED is sent on the frame that noticed: {moved:?}",
        );
        assert_eq!(
            held.as_deref(),
            Some("sprag: work \u{2014} claude idle (pane 1)"),
            "the record follows what was sent, so the next frame compares against it",
        );
    }

    /// An `idle` agent outranks a `working` one, and the pane id rides each entry.
    ///
    /// The id is the only handle this client's panes have — it paints no numbers anywhere — and it is
    /// the id `sprag panes`, `sprag agent` and the MCP tools all take, so the title tells a user what
    /// to type next.
    ///
    /// REVERT-PROOF: rank `idle` and `working` the same and the order becomes the input's, which
    /// leaves an agent that is waiting for somebody behind one that is not.
    #[test]
    fn an_agent_at_rest_outranks_one_that_is_still_working() {
        let panes = [
            (PaneId(1), agent("working", "claude")),
            (PaneId(2), agent("idle", "claude")),
        ];
        assert_eq!(
            agent_window_title("main", &panes),
            "sprag: main \u{2014} claude idle (pane 2), claude working (pane 1)",
        );
    }

    /// Paint a buffer onto a surface of its own size, as the sole focused pane — the composition
    /// every test here asserts through, so none of them assert on the change LIST when what matters
    /// is the screen.
    fn painted(grid: &GridBuffer) -> Surface {
        painted_in(
            grid,
            Rect::screen(grid.cols(), grid.rows()),
            grid.cols(),
            grid.rows(),
        )
    }

    /// Paint a buffer into `area` on a `cols` x `rows` surface, cursor and all — the multi-pane
    /// composition, with the one pane that has focus.
    fn painted_in(grid: &GridBuffer, area: Rect, cols: u16, rows: u16) -> Surface {
        let mut surface = Surface::new(usize::from(cols), usize::from(rows));
        surface.add_changes(pane_changes(grid, area, (0, 0)));
        surface.add_changes(cursor_changes(grid, area, (0, 0)));
        surface
    }

    /// A cell with a cluster and default everything else.
    fn cell(cluster: impl Into<std::borrow::Cow<'static, str>>) -> TermCell {
        TermCell::new(cluster, TermColor::Default, TermColor::Default)
    }

    /// A one-row buffer of `cells`, padded with blanks to `cols`.
    fn row(cols: u16, cells: Vec<TermCell>) -> GridBuffer {
        GridBuffer::new(cols, 1).with_row(0, cells)
    }

    /// A token over `stamps`, everything else at the value a fresh main screen carries.
    fn token(stamps: &[u64], cols: u16) -> ProjectionToken {
        ProjectionToken {
            row_generations: stamps.to_vec(),
            cursor: GridCursor::default(),
            screen: pinion_core::ScreenKind::Main,
            cols,
            scrollback_len: 0,
        }
    }

    /// A `cols` x `stamps.len()` buffer whose row `r` is `text[r]`, blank-padded.
    fn grid_of(cols: u16, text: &[&str]) -> GridBuffer {
        let mut grid = GridBuffer::new(cols, u16::try_from(text.len()).expect("a small grid"));
        for (index, line) in text.iter().enumerate() {
            grid = grid.with_row(
                u16::try_from(index).expect("a small grid"),
                line.chars()
                    .map(|c| cell(c.to_string()))
                    .collect::<Vec<_>>(),
            );
        }
        grid
    }

    /// One pane filling a surface of its own size.
    fn whole(pane: u64, grid: &GridBuffer, token: Option<ProjectionToken>) -> PanePaint {
        PanePaint {
            source: PaneSource::Direct((0, 0)),
            pane: PaneId(pane),
            area: Rect::screen(grid.cols(), grid.rows()),
            cells: grid.clone(),
            token,
        }
    }

    /// A re-wrapped pane, keyed on which re-wrapped ROW it starts at.
    fn rewrapped(
        pane: u64,
        grid: &GridBuffer,
        top: u16,
        token: Option<ProjectionToken>,
    ) -> PanePaint {
        PanePaint {
            source: PaneSource::Rewrapped { top },
            pane: PaneId(pane),
            area: Rect::screen(grid.cols(), grid.rows()),
            cells: grid.clone(),
            token,
        }
    }

    /// **A RE-WRAPPED PANE IS CACHEABLE, AND THE ANCHOR IS WHY IT IS SOUND.**
    ///
    /// Two claims, and the second is the one that took a measurement to find. A re-wrapped pane
    /// whose folded stamps have not moved is skipped like any other — that is worth ~94% of what
    /// such a pane costs per frame (`tests/rewrap_frame_cost.rs`: the change list is 1350
    /// allocations against the re-wrap's 80).
    ///
    /// And a re-wrapped pane whose VIEW SCROLLED is rebuilt even though its stamps are identical.
    /// That case is real rather than theoretical: two logical lines nobody has touched carry the
    /// same folded damage, so a view sliding by one row puts different content under the same
    /// numbers. The buffer is already cut to the rectangle, so its own coordinates start at zero
    /// however far it has scrolled — [`PaneSource::Rewrapped`]'s `top` is the only thing left that
    /// can say the frame moved.
    ///
    /// REVERT-PROOF: drop `top` from the key (make `Rewrapped` a unit variant, or read the source
    /// as `read_at`) and the second half returns an empty change list for a screen that scrolled.
    #[test]
    fn a_re_wrapped_pane_is_skipped_by_its_folded_stamps_and_rebuilt_when_the_view_scrolls() {
        // Two rows of a pane nobody has touched: the SAME folded stamp on both, which is what a
        // fold over untouched source lines produces.
        let showing = grid_of(4, &["ab", "cd"]);
        let mut cache = PaintCache::default();
        let all = cache.changes(&[rewrapped(1, &showing, 0, Some(token(&[9, 9], 4)))]);
        assert!(!all.is_empty(), "the first frame writes the pane");

        let again = cache.changes(&[rewrapped(1, &showing, 0, Some(token(&[9, 9], 4)))]);
        assert!(
            again.is_empty(),
            "nothing moved, so a re-wrapped pane costs the surface nothing: {again:?}",
        );

        // The view slides by one re-wrapped row. The stamps are IDENTICAL — they are folds over
        // lines nothing has written to — and the content is not.
        let scrolled = grid_of(4, &["cd", "ef"]);
        let moved = cache.changes(&[rewrapped(1, &scrolled, 1, Some(token(&[9, 9], 4)))]);
        assert_eq!(
            moved.len(),
            all.len(),
            "a scrolled view must be rebuilt whole, since its stamps cannot say it moved",
        );

        // ...and it must be rebuilt from the buffer's OWN ORIGIN. The anchor says which re-wrapped
        // row this buffer starts at; it is not an index into it, and reading at it would draw the
        // second row twice and lose the first.
        let mut surface = Surface::new(4, 2);
        surface.add_changes(moved);
        let painted = surface.screen_chars_to_string();
        assert!(
            painted.starts_with("cd") && painted.contains("ef"),
            "the scrolled window's own two rows, in order: {painted:?}",
        );
    }

    /// **THE claim.** A row whose stamp did not move is not written again — and the one that moved
    /// is. Asserted on the CHANGE LIST rather than the screen, because a screen that looks right
    /// cannot tell a skipped row from a rewritten one, which is the whole difference being made.
    #[test]
    fn only_the_rows_whose_stamps_moved_are_written_again() {
        let first = grid_of(4, &["ab", "cd", "ef"]);
        let mut cache = PaintCache::default();
        let all = cache.changes(&[whole(1, &first, Some(token(&[1, 1, 1], 4)))]);

        let second = grid_of(4, &["ab", "ZZ", "ef"]);
        let some = cache.changes(&[whole(1, &second, Some(token(&[1, 2, 1], 4)))]);

        assert_eq!(
            some.len() * 3,
            all.len(),
            "one row of three must cost exactly a third of three: {} vs {}",
            some.len(),
            all.len(),
        );
        let mut surface = Surface::new(4, 3);
        surface.add_changes(all);
        surface.add_changes(some);
        assert_eq!(
            surface.screen_chars_to_string(),
            painted(&second).screen_chars_to_string(),
            "and the screen must still be what a full rebuild would have drawn",
        );
    }

    /// A frame that changes NOTHING writes nothing at all — the steady state a keystroke's
    /// notification wakes every OTHER pane into.
    #[test]
    fn an_unchanged_pane_costs_no_changes_at_all() {
        let grid = grid_of(4, &["ab", "cd"]);
        let mut cache = PaintCache::default();
        let _ = cache.changes(&[whole(1, &grid, Some(token(&[7, 7], 4)))]);
        assert!(
            cache
                .changes(&[whole(1, &grid, Some(token(&[7, 7], 4)))])
                .is_empty(),
        );
    }

    /// No token means "cannot say", and the only safe reading of that is to rebuild — every frame,
    /// not just the first, so a host that never answers is exactly the client that shipped before
    /// this cache existed.
    #[test]
    fn a_pane_with_no_token_is_rebuilt_every_frame() {
        let grid = grid_of(4, &["ab", "cd"]);
        let mut cache = PaintCache::default();
        let first = cache.changes(&[whole(1, &grid, None)]);
        let second = cache.changes(&[whole(1, &grid, None)]);
        assert!(!second.is_empty());
        assert_eq!(first.len(), second.len());
    }

    /// A token that arrives AFTER one that did not must not be compared against nothing — and a
    /// pane that stops answering must not leave a token behind for a later frame to trust.
    #[test]
    fn a_pane_that_stops_answering_forgets_what_it_said() {
        let grid = grid_of(4, &["ab", "cd"]);
        let mut cache = PaintCache::default();
        let _ = cache.changes(&[whole(1, &grid, Some(token(&[1, 1], 4)))]);
        let _ = cache.changes(&[whole(1, &grid, None)]);
        assert!(
            !cache
                .changes(&[whole(1, &grid, Some(token(&[1, 1], 4)))])
                .is_empty(),
            "the token before the silence is not a description of what is on the surface now",
        );
    }

    /// The three things a row stamp cannot see, each of which must rebuild the pane WHOLE.
    ///
    /// A resize copies surviving rows' stamps, so a width change is invisible to them; an
    /// alternate-screen switch replaces the content while both screens keep their own counters; and
    /// the row count is the same argument on the other axis.
    #[test]
    fn what_the_stamps_cannot_see_rebuilds_the_pane_whole() {
        let grid = grid_of(4, &["ab", "cd"]);
        let same = token(&[1, 1], 4);

        for (what, moved) in [
            (
                "the alternate screen",
                ProjectionToken {
                    screen: pinion_core::ScreenKind::Alternate,
                    ..same.clone()
                },
            ),
            (
                "a width change",
                ProjectionToken {
                    cols: 5,
                    ..same.clone()
                },
            ),
            (
                "a row count change",
                ProjectionToken {
                    row_generations: vec![1, 1, 1],
                    ..same.clone()
                },
            ),
        ] {
            let mut cache = PaintCache::default();
            let _ = cache.changes(&[whole(1, &grid, Some(same.clone()))]);
            assert!(
                !cache.changes(&[whole(1, &grid, Some(moved))]).is_empty(),
                "{what} must not be skipped over",
            );
        }
    }

    /// The cache describes a SCREEN, so a changed arrangement discards it — even for a pane whose
    /// own rectangle and stamps are untouched.
    ///
    /// **Read off the surface, because that is the only place the defect is visible.** A split and
    /// its undo return a pane to a rectangle it held before, with the same content and therefore
    /// the same stamps — while the pane that sat beside it in between has written over half of it.
    /// A cache comparing only that pane's own token would skip the rebuild and leave the neighbour's
    /// text on screen, and every count in the frame would still look plausible: the assertion has
    /// to be what the terminal SHOWS.
    ///
    /// The first version of this test asserted the frame was non-empty and passed with the
    /// arrangement check deleted, because the joining pane's own changes were in the same list.
    /// **A VIEW THAT SCROLLS OVER UNCHANGED CONTENT IS STILL A NEW FRAME.**
    ///
    /// The case the arrangement key exists for, and the one no end-to-end gate can reach: a pane
    /// bigger than the client's viewport keeps the SAME rectangle at every offset, and its rows keep
    /// the same stamps while nothing writes to it. So a cache keyed on the rectangle and the stamps
    /// alone would answer "nothing to do" for a frame showing entirely different cells — and it
    /// would be right about both of the things it was looking at. Moving focus over static output is
    /// exactly that frame; the mutation that drops `from` from the key was GREEN against the whole
    /// suite before this existed.
    #[test]
    fn a_view_that_scrolled_rewrites_the_pane_although_nothing_in_it_changed() {
        let grid = grid_of(4, &["one", "two", "three", "four"]);
        let token = token(&[7, 8, 9, 10], 4);
        let area = Rect::screen(4, 2);
        let mut cache = PaintCache::default();

        let first = cache.changes(&[PanePaint {
            pane: PaneId(1),
            area,
            source: PaneSource::Direct((0, 0)),
            cells: grid.clone(),
            token: Some(token.clone()),
        }]);
        assert!(!first.is_empty(), "the first frame writes the pane");
        assert!(
            cache
                .changes(&[PanePaint {
                    pane: PaneId(1),
                    area,
                    source: PaneSource::Direct((0, 0)),
                    cells: grid.clone(),
                    token: Some(token.clone()),
                }])
                .is_empty(),
            "the same view of the same cells owes nothing",
        );

        // The view scrolls two rows down it. Same rectangle, same stamps, different CELLS.
        let scrolled = cache.changes(&[PanePaint {
            pane: PaneId(1),
            area,
            source: PaneSource::Direct((0, 2)),
            cells: grid.clone(),
            token: Some(token.clone()),
        }]);
        assert!(
            !scrolled.is_empty(),
            "a scrolled view shows different cells and owes the surface every one of them",
        );

        // And what it wrote is the rows it scrolled TO, not the ones it left.
        let mut surface = Surface::new(4, 2);
        surface.add_changes(scrolled);
        assert_eq!(
            surface.screen_chars_to_string().trim_end(),
            "three\nfour".replace("three", "thre"),
            "the pane's rows 2 and 3, cut to the rectangle's width",
        );
    }

    #[test]
    fn a_changed_arrangement_discards_the_cache() {
        let alone = grid_of(4, &["aaaa", "aaaa"]);
        let stamps = token(&[1, 1], 4);
        let left = grid_of(2, &["LL", "LL"]);
        let right = grid_of(2, &["RR", "RR"]);
        let half = |col: u16| Rect {
            col,
            row: 0,
            cols: 2,
            rows: 2,
        };

        let mut cache = PaintCache::default();
        let mut surface = Surface::new(4, 2);
        // Alone over the whole screen...
        surface.add_changes(cache.changes(&[whole(1, &alone, Some(stamps.clone()))]));
        // ...then split, so a neighbour owns the right half...
        surface.add_changes(cache.changes(&[
            PanePaint {
                source: PaneSource::Direct((0, 0)),
                pane: PaneId(1),
                area: half(0),
                cells: left.clone(),
                token: Some(stamps.clone()),
            },
            PanePaint {
                source: PaneSource::Direct((0, 0)),
                pane: PaneId(2),
                area: half(2),
                cells: right,
                token: Some(stamps.clone()),
            },
        ]));
        // ...and then alone again, unchanged, which is where a per-pane comparison goes wrong.
        surface.add_changes(cache.changes(&[whole(1, &alone, Some(stamps))]));

        assert_eq!(
            surface.screen_chars_to_string(),
            painted(&alone).screen_chars_to_string(),
            "the neighbour's cells must not survive the pane's return to the whole screen",
        );
    }

    /// A rectangle TALLER than the pane's stamps is written past them, not indexed past them.
    ///
    /// The rectangle is the authority and the grid catches up to a resize a wake behind it, so a
    /// pane routinely owns rows its own buffer does not have. Those rows are outside anything the
    /// stamps vouch for and are always rebuilt — and the guard that says so is also what keeps a
    /// row index from running off the end of the token, which is why this test drives four rows
    /// against two stamps rather than asserting on a count.
    #[test]
    fn a_rectangle_taller_than_the_stamps_is_written_not_indexed() {
        let grid = grid_of(4, &["ab", "cd"]);
        let tall = Rect {
            col: 0,
            row: 0,
            cols: 4,
            rows: 4,
        };
        let stamps = token(&[1, 1], 4);
        let mut cache = PaintCache::default();
        let mut surface = Surface::new(4, 4);
        for _ in 0..2 {
            surface.add_changes(cache.changes(&[PanePaint {
                source: PaneSource::Direct((0, 0)),
                pane: PaneId(1),
                area: tall,
                cells: grid.clone(),
                token: Some(stamps.clone()),
            }]));
        }
        let mut whole_every_time = Surface::new(4, 4);
        whole_every_time.add_changes(pane_changes(&grid, tall, (0, 0)));
        assert_eq!(
            surface.screen_chars_to_string(),
            whole_every_time.screen_chars_to_string(),
        );
    }

    /// A blanked surface holds nothing, so the cache must not answer as if it did.
    #[test]
    fn forgetting_makes_the_next_frame_whole() {
        let grid = grid_of(4, &["ab", "cd"]);
        let stamps = token(&[1, 1], 4);
        let mut cache = PaintCache::default();
        let full = cache.changes(&[whole(1, &grid, Some(stamps.clone()))]);
        cache.forget();
        assert_eq!(
            cache.changes(&[whole(1, &grid, Some(stamps))]).len(),
            full.len(),
        );
    }

    /// **The end-to-end safety claim, read off a screen.** A surface driven through a run of frames
    /// THROUGH the cache holds exactly what a surface rebuilt in full every frame holds.
    ///
    /// Written this way because the failure this guards is invisible from any single frame: a
    /// wrongly skipped row shows the PREVIOUS frame's text, which is a perfectly plausible screen
    /// until it is compared with the one that was owed.
    #[test]
    fn a_cached_run_of_frames_paints_what_a_full_rebuild_paints() {
        let frames = [
            (grid_of(6, &["one", "two", "six"]), token(&[1, 1, 1], 6)),
            (grid_of(6, &["one", "TWO", "six"]), token(&[1, 2, 1], 6)),
            (grid_of(6, &["ONE", "TWO", "six"]), token(&[2, 2, 1], 6)),
            (grid_of(6, &["ONE", "TWO", "SIX"]), token(&[2, 2, 9], 6)),
            (grid_of(6, &["ONE", "TWO", "SIX"]), token(&[2, 2, 9], 6)),
        ];
        let mut cache = PaintCache::default();
        let mut cached = Surface::new(6, 3);
        let mut whole_every_time = Surface::new(6, 3);
        for (grid, stamps) in &frames {
            cached.add_changes(cache.changes(&[whole(1, grid, Some(stamps.clone()))]));
            whole_every_time.add_changes(pane_changes(grid, Rect::screen(6, 3), (0, 0)));
            assert_eq!(
                cached.screen_chars_to_string(),
                whole_every_time.screen_chars_to_string(),
            );
        }
    }

    /// The base case, and the one every other test rests on: clusters land in their own columns.
    #[test]
    fn text_lands_in_its_own_columns() {
        let grid = row(6, "hi".chars().map(|c| cell(c.to_string())).collect());
        assert_eq!(painted(&grid).screen_chars_to_string().trim_end(), "hi");
    }

    /// The colour vocabulary round-trips through all three forms. `Rgb` is what a host actually
    /// sends (the projection resolves the palette), so it is the one that must be exact; the other
    /// two are what a hand-built buffer carries.
    #[test]
    fn the_three_colour_forms_each_have_an_exact_peer() {
        assert_eq!(term_color(TermColor::Default), ColorAttribute::Default);
        assert_eq!(
            term_color(TermColor::Indexed(9)),
            ColorAttribute::PaletteIndex(9)
        );
        assert_eq!(
            term_color(TermColor::Rgb(PinColor::rgb(0xcd, 0x00, 0x00))),
            ColorAttribute::TrueColorWithDefaultFallback(SrgbaTuple::from((
                0xcd_u8, 0x00_u8, 0x00_u8, 0xff_u8
            ))),
        );
    }

    /// Bold and dim collapse onto ONE termwiz axis, deterministically. The pair is unreachable
    /// through sprag's emulator (which clears one when it sets the other), but the function is
    /// total over a type that admits it, so the rule is pinned rather than left to argument order.
    #[test]
    fn intensity_collapses_with_bold_winning() {
        let bold = cell("x").with_attrs(CellAttrs::empty().with_bold(true));
        let dim = cell("x").with_attrs(CellAttrs::empty().with_dim(true));
        let both = cell("x").with_attrs(CellAttrs::empty().with_bold(true).with_dim(true));
        assert_eq!(cell_attributes(&bold, None).intensity(), Intensity::Bold);
        assert_eq!(cell_attributes(&dim, None).intensity(), Intensity::Half);
        assert_eq!(cell_attributes(&both, None).intensity(), Intensity::Bold);
        assert_eq!(
            cell_attributes(&cell("x"), None).intensity(),
            Intensity::Normal
        );
    }

    /// All six SGR 4:x styles survive. A backend that folded them to one "underlined" bit would
    /// pass a test that only checked `Single`, so every variant is named.
    #[test]
    fn every_underline_style_survives() {
        for (pin, tw) in [
            (PinUnderlineStyle::None, Underline::None),
            (PinUnderlineStyle::Single, Underline::Single),
            (PinUnderlineStyle::Double, Underline::Double),
            (PinUnderlineStyle::Curly, Underline::Curly),
            (PinUnderlineStyle::Dotted, Underline::Dotted),
            (PinUnderlineStyle::Dashed, Underline::Dashed),
        ] {
            assert_eq!(underline(pin), tw, "{pin:?} must map to {tw:?}");
        }
    }

    /// A wide cluster occupies two COLUMNS and its trailer prints nothing — so the cell after the
    /// pair lands where the producer said it would. This is the alignment invariant the whole
    /// width cross-check exists to protect.
    ///
    /// Asserted through `screen_cells` rather than `screen_chars_to_string`, and the distinction
    /// is not pedantry: the string form emits ONE char for a wide cluster, so `"한!"` and a
    /// correctly-placed `!` look identical to a `!` wrongly written at column 1. Only the cell
    /// grid distinguishes them, which is the same reason `sprag-grid` asserts cells.
    #[test]
    fn a_wide_cluster_occupies_two_columns_and_its_trailer_prints_nothing() {
        let wide = TermCell::new("한", TermColor::Default, TermColor::Default).wide();
        let grid = row(6, vec![wide.clone(), wide.trailer(), cell("!")]);
        let mut surface = painted(&grid);
        let cells = surface.screen_cells();
        assert_eq!(cells[0][0].str(), "한");
        assert_eq!(
            cells[0][2].str(),
            "!",
            "column 2, not the trailer's column 1"
        );
    }

    /// Two wide clusters in a ROW keep their columns. The single-wide case cannot catch a
    /// double-counted span — the error only displaces from the SECOND cluster on — and the
    /// emulator-driven battery caught exactly that while this module's single-cluster test passed.
    /// Pinned here so the cheap suite is not blind to it either.
    #[test]
    fn consecutive_wide_clusters_keep_their_columns() {
        let wide = |cluster: &'static str| {
            TermCell::new(cluster, TermColor::Default, TermColor::Default).wide()
        };
        let (first, second) = (wide("가"), wide("나"));
        let grid = row(
            8,
            vec![
                first.clone(),
                first.trailer(),
                second.clone(),
                second.trailer(),
                cell("|"),
            ],
        );
        let mut surface = painted(&grid);
        let cells = surface.screen_cells();
        assert_eq!(cells[0][0].str(), "가");
        assert_eq!(cells[0][2].str(), "나", "the second head, not column 1");
        assert_eq!(cells[0][4].str(), "|");
    }

    /// **THE WIDTH-DISAGREEMENT GUARD, proven.** A cell declaring itself `Narrow` while carrying a
    /// cluster termwiz measures as two columns IS the disagreement the cross-check exists for,
    /// expressed directly — the same shape a divergence between `unicode-width` and
    /// `widechar_width` would take, without having to find a character the two currently disagree
    /// on (the emulator-driven battery found none, including ZWJ families and ambiguous-width
    /// characters).
    ///
    /// REVERT-PROOF, measured: delete the `unicode_column_width` check in [`pane_changes`] and
    /// this fails — `c` lands at column 2 instead of column 1, and in a full row every remaining
    /// cell would follow it. That is the failure the guard buys, and it is a whole line of garbage
    /// from one character.
    #[test]
    fn a_cluster_wider_than_its_declared_width_does_not_displace_the_rest_of_the_row() {
        // Declared Narrow (the default), cluster two columns wide.
        let grid = row(8, vec![cell("한"), cell("c"), cell("d")]);
        let mut surface = painted(&grid);
        let cells = surface.screen_cells();
        assert_eq!(
            cells[0][1].str(),
            "c",
            "re-anchored, not pushed to column 2"
        );
        assert_eq!(cells[0][2].str(), "d");
    }

    /// A trailer with no head before it is a malformed buffer — nothing the projection makes, but
    /// constructible, and skipping it would shift the rest of the row left. It is rendered as the
    /// blank column it occupies.
    ///
    /// **The orphan must sit INSIDE a run, not at the head of one.** The first version of this
    /// test put it at column 0 and passed with the guard removed, because a skipped FIRST cell
    /// leaves the run empty and the next cell restarts it at its own absolute column — masking the
    /// displacement completely. A cell of the same attributes before it is what makes the run
    /// continue across the gap, which is the only arrangement in which the bug exists.
    ///
    /// REVERT-PROOF, measured: make the orphan arm of [`printed`] return `None` and this fails
    /// with `b` at column 1.
    #[test]
    fn an_orphan_trailer_holds_its_column_instead_of_vanishing() {
        let orphan = TermCell::new("", TermColor::Default, TermColor::Default).trailer();
        let grid = row(6, vec![cell("a"), orphan, cell("b")]);
        let mut surface = painted(&grid);
        let cells = surface.screen_cells();
        assert_eq!(cells[0][1].str(), " ", "the orphan's own column, blanked");
        assert_eq!(cells[0][2].str(), "b", "and nothing after it moved");
    }

    /// A run ends when the attributes change, so two differently-styled halves of a row keep their
    /// own styles instead of the first winning.
    #[test]
    fn attribute_changes_split_a_row_into_runs() {
        let plain = cell("a");
        let bold = cell("b").with_attrs(CellAttrs::empty().with_bold(true));
        let mut surface = painted(&row(4, vec![plain, bold]));
        let cells = surface.screen_cells();
        assert_eq!(cells[0][0].attrs().intensity(), Intensity::Normal);
        assert_eq!(cells[0][1].attrs().intensity(), Intensity::Bold);
    }

    /// An empty cluster still occupies its column. Without the blank substitution the rest of the
    /// row would silently shift left, which is the failure mode hardest to spot in a screenshot
    /// and trivial to spot here.
    #[test]
    fn an_empty_cluster_still_occupies_its_column() {
        let grid = row(4, vec![cell("a"), cell(""), cell("c")]);
        let screen = painted(&grid).screen_chars_to_string();
        assert_eq!(screen.chars().nth(2), Some('c'));
    }

    /// The cursor's shape and blink mode fold onto termwiz's single enum without loss, and the
    /// position is the pane's own.
    #[test]
    fn the_cursor_shape_and_blink_fold_onto_one_enum() {
        for (shape, blink, expected) in [
            (PinCursorShape::Block, false, CursorShape::SteadyBlock),
            (PinCursorShape::Block, true, CursorShape::BlinkingBlock),
            (PinCursorShape::Bar, false, CursorShape::SteadyBar),
            (PinCursorShape::Bar, true, CursorShape::BlinkingBar),
            (
                PinCursorShape::Underline,
                false,
                CursorShape::SteadyUnderline,
            ),
            (
                PinCursorShape::Underline,
                true,
                CursorShape::BlinkingUnderline,
            ),
        ] {
            let cursor = GridCursor::new(1, 0, shape, true).with_blink(blink);
            let grid = GridBuffer::new(4, 1).with_cursor(cursor);
            let surface = painted(&grid);
            assert_eq!(surface.cursor_shape(), Some(expected));
            assert_eq!(surface.cursor_position(), (1, 0));
            assert_eq!(surface.cursor_visibility(), CursorVisibility::Visible);
        }
    }

    /// A cursor the producer has placed outside the buffer — which pinion's docs say happens
    /// during an in-flight resize — is reported HIDDEN rather than clamped, so no authoritative
    /// block is drawn in a cell nobody named.
    #[test]
    fn an_out_of_bounds_cursor_is_hidden_not_clamped() {
        let cursor = GridCursor::new(9, 0, PinCursorShape::Block, true);
        let surface = painted(&GridBuffer::new(4, 1).with_cursor(cursor));
        assert_eq!(surface.cursor_visibility(), CursorVisibility::Hidden);
    }

    /// A rectangle with no cells paints nothing at all — a claim about a screen that does not
    /// exist. Reachable: the layouter hands one out for a terminal that reports no size.
    ///
    /// An empty BUFFER is the opposite case and is asserted beside it: a pane whose first frame has
    /// not arrived still owns its rectangle, so it paints — blank.
    #[test]
    fn an_empty_rectangle_paints_nothing_but_an_empty_buffer_still_blanks_its_own() {
        assert!(pane_changes(&GridBuffer::new(4, 1), Rect::screen(0, 0), (0, 0)).is_empty());
        assert!(!pane_changes(&GridBuffer::new(0, 0), Rect::screen(4, 1), (0, 0)).is_empty());
    }

    /// A pane paints at its OWN origin, not the screen's — the whole of what multi-pane adds to the
    /// mapping, and the thing a single-pane test can never catch because there the two coincide.
    ///
    /// REVERT-PROOF, measured: drop the `area.col +` from the run's restart and `hi` lands at
    /// column 0 of row 0 — on top of whichever pane owns the screen's top-left corner.
    #[test]
    fn a_pane_paints_at_its_own_origin() {
        let grid = row(2, "hi".chars().map(|c| cell(c.to_string())).collect());
        let mut surface = painted_in(&grid, Rect::new(10, 3, 2, 1), 20, 5);
        let cells = surface.screen_cells();
        assert_eq!(cells[3][10].str(), "h");
        assert_eq!(cells[3][11].str(), "i");
        assert_eq!(
            cells[0][0].str(),
            " ",
            "and nothing was written at the origin"
        );
    }

    /// A grid the arrangement has outgrown BLANKS the rest of its rectangle. Without it the cells
    /// the pane no longer covers keep the previous frame — which after a split is the other pane's
    /// content, sitting inside this pane's border until the resize catches up.
    ///
    /// REVERT-PROOF, measured: make the out-of-buffer arm `continue` instead of blanking and the
    /// planted `XXXX` survives the paint.
    #[test]
    fn a_grid_shorter_than_its_rectangle_blanks_the_remainder() {
        let mut surface = Surface::new(8, 2);
        surface.add_change(Change::Text("XXXXXXXX".to_owned()));
        // A 2x1 pane painted into a 4x2 rectangle: six of the eight cells are the buffer's absence.
        surface.add_changes(pane_changes(
            &row(2, vec![cell("o"), cell("k")]),
            Rect::screen(4, 2),
            (0, 0),
        ));
        let cells = surface.screen_cells();
        assert_eq!(cells[0][0].str(), "o");
        assert_eq!(cells[0][2].str(), " ", "past the buffer's last column");
        assert_eq!(cells[0][3].str(), " ");
        assert_eq!(cells[1][0].str(), " ", "past the buffer's last row");
        assert_eq!(
            cells[0][4].str(),
            "X",
            "and nothing outside the rectangle moved"
        );
    }

    /// A grid the rectangle has outgrown is CLIPPED: the cells past the edge are not written, so
    /// they stay whatever their real owner put there.
    #[test]
    fn a_grid_larger_than_its_rectangle_is_clipped() {
        let mut surface = Surface::new(6, 1);
        surface.add_change(Change::Text("......".to_owned()));
        let grid = row(6, "abcdef".chars().map(|c| cell(c.to_string())).collect());
        surface.add_changes(pane_changes(&grid, Rect::screen(3, 1), (0, 0)));
        let cells = surface.screen_cells();
        assert_eq!(cells[0][2].str(), "c", "the last column inside");
        assert_eq!(
            cells[0][3].str(),
            ".",
            "and the first one outside is untouched"
        );
    }

    /// **THE EDGE GUARD.** A wide cluster in the rectangle's last column is blanked rather than
    /// drawn, because its second column is the divider's cell — and a glyph written there is a
    /// pane bleeding through the line that is supposed to contain it.
    ///
    /// REVERT-PROOF, measured: delete the `columns > room` arm and column 2 holds `한` while column
    /// 3 — outside the pane entirely — goes from the planted `.` to the cluster's trailing half.
    /// Both cells are wrong, and the second is in someone else's rectangle.
    #[test]
    fn a_wide_cluster_at_the_right_edge_is_blanked_rather_than_bleeding() {
        let wide = TermCell::new("한", TermColor::Default, TermColor::Default).wide();
        let grid = row(4, vec![cell("a"), cell("b"), wide.clone(), wide.trailer()]);
        let mut surface = Surface::new(4, 1);
        surface.add_change(Change::Text("....".to_owned()));
        surface.add_changes(pane_changes(&grid, Rect::screen(3, 1), (0, 0)));
        let cells = surface.screen_cells();
        assert_eq!(
            cells[0][2].str(),
            " ",
            "the cluster does not fit, so it is not drawn"
        );
        assert_eq!(
            cells[0][3].str(),
            ".",
            "and the cell beyond the pane is untouched"
        );
    }

    /// The focused pane's cursor lands at the pane's origin plus its own position — so the terminal
    /// cursor rests where the user is typing, in the pane they are typing into.
    ///
    /// REVERT-PROOF, measured: drop the `area.col +` / `area.row +` from the cursor's position and
    /// it comes to rest at (1, 0) — inside whichever pane holds the screen's corner.
    #[test]
    fn the_cursor_lands_at_the_panes_own_origin() {
        let cursor = GridCursor::new(1, 0, PinCursorShape::Block, true);
        let grid = GridBuffer::new(4, 2).with_cursor(cursor);
        let surface = painted_in(&grid, Rect::new(6, 4, 4, 2), 20, 8);
        assert_eq!(surface.cursor_position(), (7, 4));
        assert_eq!(surface.cursor_visibility(), CursorVisibility::Visible);
    }

    /// A cursor the producer has placed outside the pane's RECTANGLE is hidden, not drawn over the
    /// neighbour it would land in. Reachable during a resize: the grid still carries the old size's
    /// cursor while the rectangle has already shrunk.
    ///
    /// REVERT-PROOF, measured: drop the `cursor.col < area.cols` bound and the surface reports
    /// `Visible` at `(6, 0)` — two columns past a four-column pane, inside the pane next door.
    #[test]
    fn a_cursor_outside_the_rectangle_is_hidden() {
        let cursor = GridCursor::new(6, 0, PinCursorShape::Block, true);
        let grid = GridBuffer::new(8, 1).with_cursor(cursor);
        let surface = painted_in(&grid, Rect::new(0, 0, 4, 1), 20, 4);
        assert_eq!(surface.cursor_visibility(), CursorVisibility::Hidden);
    }

    /// A HORIZONTAL split lays its panes side by side, so the line between them is VERTICAL — the
    /// inversion the host's and tmux's `-h` vocabulary carries, asserted rather than argued.
    #[test]
    fn a_horizontal_splits_divider_is_a_vertical_line() {
        let divider = Divider {
            area: Rect::new(2, 0, 1, 3),
            dir: SplitDir::Horizontal,
            id: None,
            region: Rect::screen(4, 3),
        };
        let mut surface = Surface::new(4, 3);
        surface.add_changes(divider_changes(&divider));
        let cells = surface.screen_cells();
        for (row, line) in cells.iter().enumerate() {
            assert_eq!(line[2].str(), "\u{2502}", "row {row}");
            assert_eq!(line[1].str(), " ", "and only its own column");
        }
    }

    /// ...and a VERTICAL split's divider is a horizontal line, spanning its region's width.
    #[test]
    fn a_vertical_splits_divider_is_a_horizontal_line() {
        let divider = Divider {
            area: Rect::new(0, 1, 4, 1),
            dir: SplitDir::Vertical,
            id: None,
            region: Rect::screen(4, 3),
        };
        let mut surface = Surface::new(4, 3);
        surface.add_changes(divider_changes(&divider));
        assert_eq!(
            surface.screen_chars_to_string().lines().nth(1),
            Some("\u{2500}\u{2500}\u{2500}\u{2500}"),
        );
    }

    /// **THE COMPOSITION RULE, executable.** With two panes on screen, the cursor comes to rest in
    /// the FOCUSED one — even when the other pane paints after it.
    ///
    /// This is the ordering [`cursor_changes`]'s docs state, and the reason it needs a test rather
    /// than a comment: [`Change::Text`] moves the surface's cursor as it writes, so a composition
    /// that emitted the focused pane's cursor while walking the panes would leave it trailing
    /// whichever pane came last. The focused pane here is deliberately the FIRST one, which is the
    /// arrangement where the bug exists — with the focused pane last, a wrong composition and a
    /// right one agree.
    #[test]
    fn the_cursor_rests_in_the_focused_pane_even_when_another_paints_after_it() {
        let focused = row(2, vec![cell("a"), cell("b")]).with_cursor(GridCursor::new(
            1,
            0,
            PinCursorShape::Block,
            true,
        ));
        let other = row(2, vec![cell("y"), cell("z")]);
        let (left, right) = (Rect::new(0, 0, 2, 1), Rect::new(3, 0, 2, 1));
        let mut surface = Surface::new(5, 1);
        // The composition the client makes: every pane's cells, then the focused pane's cursor.
        surface.add_changes(pane_changes(&focused, left, (0, 0)));
        surface.add_changes(pane_changes(&other, right, (0, 0)));
        surface.add_changes(cursor_changes(&focused, left, (0, 0)));
        assert_eq!(surface.cursor_position(), (1, 0));
        // ...and the wrong order, to show the assertion above is not vacuous: emitting the cursor
        // before the other pane's cells leaves it wherever that pane's text ended.
        let mut wrong = Surface::new(5, 1);
        wrong.add_changes(pane_changes(&focused, left, (0, 0)));
        wrong.add_changes(cursor_changes(&focused, left, (0, 0)));
        wrong.add_changes(pane_changes(&other, right, (0, 0)));
        assert_eq!(wrong.cursor_position(), (5, 0), "trailing the other pane");
    }

    /// A divider carries its OWN attributes rather than inheriting the last pane's run — a line
    /// that picked up a program's reverse-video would read as a selection.
    #[test]
    fn a_divider_does_not_inherit_the_last_panes_attributes() {
        let reverse = cell("x").with_attrs(CellAttrs::empty().with_reverse(true));
        let mut surface = Surface::new(3, 1);
        surface.add_changes(pane_changes(
            &row(2, vec![reverse.clone(), reverse]),
            Rect::screen(2, 1),
            (0, 0),
        ));
        surface.add_changes(divider_changes(&Divider {
            area: Rect::new(2, 0, 1, 1),
            dir: SplitDir::Horizontal,
            id: None,
            region: Rect::screen(3, 1),
        }));
        let cells = surface.screen_cells();
        assert!(
            cells[0][0].attrs().reverse(),
            "the pane's own cells keep it"
        );
        assert!(!cells[0][2].attrs().reverse(), "the divider does not");
    }
}

#[cfg(test)]
mod status_tests {
    use super::*;
    use sprag_terminal::WindowInfo;
    use termwiz::surface::Surface;

    fn status(session: &str, windows: &[(&str, bool)]) -> Status {
        Status {
            session: session.to_owned(),
            windows: windows
                .iter()
                .enumerate()
                .map(|(i, (name, current))| WindowInfo {
                    name: (*name).to_owned(),
                    // By POSITION: this surface paints a tab strip and addresses nothing, so the
                    // id is here only because the type carries one.
                    id: Some(sprag_terminal::WindowId(i as u64)),
                    current: *current,
                    opened_by: None,
                })
                .collect(),
        }
    }

    /// What the surface actually holds after `changes` are applied to an `cols` x `rows` screen.
    fn painted(cols: usize, rows: usize, changes: Vec<Change>) -> Vec<String> {
        let mut surface = Surface::new(cols, rows);
        surface.add_changes(changes);
        surface
            .screen_cells()
            .iter()
            .map(|row| row.iter().map(|cell| cell.str().to_owned()).collect())
            .collect()
    }

    /// The cut: the panes get every row but the last, and the last is where the client speaks.
    #[test]
    fn the_screen_is_cut_into_panes_and_one_row_to_speak_in() {
        let split = Split::of(80, 24);
        assert_eq!(split.panes, Rect::screen(80, 23));
        assert_eq!(split.status, Rect::new(0, 23, 80, 1));
        assert_eq!(
            split.terminal(),
            Rect::screen(80, 24),
            "the two halves add back up to what the client can draw on",
        );
    }

    /// A terminal with no room for both gives the row up rather than the panes.
    ///
    /// REVERT-PROOF: take the guard out and `rows - 1` underflows to 65535 on a one-row terminal —
    /// or, with a saturating subtraction instead, leaves the panes a rectangle of no rows at all.
    #[test]
    fn a_terminal_too_small_for_a_status_row_keeps_its_panes() {
        let one = Split::of(80, 1);
        assert_eq!(one.panes, Rect::screen(80, 1), "the pane keeps the row");
        assert!(
            one.status.is_empty(),
            "and there is no row left to speak in"
        );
        assert!(
            status_changes(one.status, &status("0", &[("0", true)]), None, None).is_empty(),
            "an empty status rectangle paints nothing, so no call site needs a branch",
        );
    }

    /// The steady state: where the client is, in the shape a tmux user already reads.
    #[test]
    fn the_row_says_where_the_client_is() {
        let split = Split::of(20, 3);
        let rows = painted(
            20,
            3,
            status_changes(
                split.status,
                &status("work", &[("0", true), ("logs", false)]),
                None,
                None,
            ),
        );
        assert_eq!(rows[2].trim_end(), "[work] 0:0* 1:logs");
        assert_eq!(
            rows[0].trim_end(),
            "",
            "and it touches no row a pane was given",
        );
    }

    /// A message REPLACES the line, which is tmux's own behaviour: half a location beside half a
    /// refusal is two truncated facts on one row.
    #[test]
    fn a_message_takes_the_row_over() {
        let split = Split::of(30, 3);
        let rows = painted(
            30,
            3,
            status_changes(
                split.status,
                &status("work", &[("0", true)]),
                Some("no session called \"ghost\""),
                None,
            ),
        );
        assert_eq!(rows[2].trim_end(), "no session called \"ghost\"");
        assert!(
            !rows[2].contains("[work]"),
            "the location is not squeezed in beside it: {:?}",
            rows[2],
        );
    }

    /// **The bottom-RIGHT cell is never written**, because a character there leaves the terminal
    /// with a pending wrap and the next write scrolls the screen out from under this row.
    ///
    /// REVERT-PROOF: drop the `saturating_sub(1)` and the last cell fills, which is what a live
    /// client did before this bound existed — the whole screen scrolled away and left one status
    /// line stranded a row too high.
    #[test]
    fn the_last_cell_of_the_bottom_row_is_left_alone() {
        let split = Split::of(8, 2);
        let rows = painted(
            8,
            2,
            status_changes(split.status, &status("0", &[]), Some("0123456789"), None),
        );
        assert_eq!(
            rows[1], "0123456 ",
            "seven cells of the message and the corner left blank",
        );
    }

    /// The row is CHROME, not a pane's last line — reverse video, the one decoration available in
    /// every terminal this client can be attached from.
    #[test]
    fn the_row_reads_as_chrome() {
        let split = Split::of(12, 2);
        let mut surface = Surface::new(12, 2);
        surface.add_changes(status_changes(split.status, &status("0", &[]), None, None));
        let cells = surface.screen_cells();
        assert!(
            cells[1][0].attrs().reverse(),
            "the status row is drawn in reverse video",
        );
        assert!(
            !cells[0][0].attrs().reverse(),
            "...and the pane rows above it are not",
        );
    }
}
