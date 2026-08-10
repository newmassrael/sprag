//! What part of the WINDOW this client is looking at.
//!
//! A session's window is arbitrated across every client watching it, so a client can be given a
//! window bigger than its own terminal — that is what `window-size = "largest"` means, and it is the
//! ordinary case the moment a phone attaches beside a desktop. The cells past this terminal's edge
//! have to go somewhere, and there are only two honest answers: CLIP them, or write them anyway and
//! let the small terminal wrap them, which displaces every row below and pushes the status line off
//! the screen. sprag clips, and has since R345 pinned it.
//!
//! ⚠ **R349 ADDED A THIRD ANSWER FOR THE PANE THAT DOES NOT FIT**, and it is neither of those two:
//! [`sprag_grid::rewrap`] rebuilds the pane's logical LINES and cuts them to the width this client
//! can show, so the row below holds the rest of the same line rather than the displaced start of
//! the next one. The status row survives, the arrangement does not move, and nothing about the
//! pane changes. It is per-pane, off on the alternate screen, and off entirely under
//! `rewrap = "off"`; everything below still describes what happens to a pane that is NOT re-wrapped
//! — and to the window, always, since a window is a layout and only a pane has lines.
//!
//! ## What clipping alone left, measured
//!
//! An 80x10 client watching an 80x23 window, two panes stacked:
//!
//! * its screen was **blank** — the pane it could show was empty, and the pane with the content
//!   began twelve rows below its last row;
//! * **its keystrokes went into that pane anyway**, because the session's active pane is session
//!   state and a client follows it. The person typed and nothing happened, on either half of the
//!   screen they could see;
//! * its status row was **byte-identical** to the row on the client that could see everything.
//!
//! So the clip was the correct half of an answer whose other half was missing. A [`Viewport`] is
//! that half: the window cell this terminal's top-left corner shows, moved so that **the pane being
//! typed into is always on screen**.
//!
//! ## Why the view follows FOCUS rather than a pan key
//!
//! Because focus is what the person is already moving. A pane the client cannot show is one they
//! reach by selecting it — the keys they already have, the ones another client's `select-pane`
//! moves too — and the view goes there the way an editor scrolls to its cursor. A pan verb would be
//! a second way to say the same thing, and the round that would have added it is the round after
//! the one that finds them disagreeing.
//!
//! ⚠ **It is a VIEWPORT and never a re-layout**, and that is a fact about pseudoterminals rather
//! than a preference: a pane is a process on a pty, a pty has exactly ONE winsize, and a program on
//! the alternate screen owns absolute cell positions at the width it was told. R346 recorded the
//! whole argument. Nothing here resizes anything.

use sprag_terminal::Rect;

/// Which row of a RE-WRAPPED pane the top of this client's `on_screen` rows shows.
///
/// A pane re-wrapped into a narrower width ([`sprag_grid::rewrap`]) is TALLER than the rectangle
/// the tiling gave it — that is what re-wrapping means — so a client shows part of it, and this is
/// which part. The [`Viewport`] above answers the same question about the WINDOW; this answers it
/// about one pane's own content, and the two are separate because a window's rows are a layout
/// while these are a person's lines.
///
/// **The pane's own first row, moved down by the LEAST that keeps the cursor on screen** — the
/// rule [`Viewport::follow`] scrolls the window by, applied to one pane's re-wrapped lines.
///
/// Bottom-anchoring was written first and MEASURED WRONG. A 100-column pane holding one wrapped
/// line and twenty-two blank rows re-wraps to twenty-four rows on a twenty-three-row client, and
/// anchoring at the bottom drops the row that is over the limit — which is the first HALF of the
/// only line on the screen, to make room for a blank one. The pane's trailing blanks are as real
/// as its text and there are usually more of them, so "the end of the content" is not where the
/// bottom is.
///
/// `cursor` is `None` for a frame that has none — a scrolled-back history window, whose last row
/// IS its newest — and that arm alone anchors at the bottom.
#[must_use]
pub const fn first_row(tall: u16, on_screen: u16, cursor: Option<u16>) -> u16 {
    let deepest = tall.saturating_sub(on_screen);
    let top = match cursor {
        Some(at) => at.saturating_sub(on_screen.saturating_sub(1)),
        None => deepest,
    };
    if top > deepest { deepest } else { top }
}

/// Where a pane's rectangle sits on THIS terminal, and which of the pane's own cells that is.
///
/// The two travel together because either alone is a way to paint the wrong thing: an area with no
/// source offset draws a pane's first column in a column the viewport has scrolled past, and an
/// offset with no area says where to read and not where to write. [`Viewport::show`] is the only
/// thing that builds one.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct PaneView {
    /// Where to draw, in this terminal's coordinates — already clipped to the screen.
    pub area: Rect,
    /// The pane's own `(col, row)` that [`area`](Self::area)'s top-left corner shows. `(0, 0)` for a
    /// pane the viewport has not scrolled past, which is every pane whenever the window fits.
    pub from: (u16, u16),
}

/// The part of an arbitrated window one client is showing.
///
/// Cheap and derived: built fresh from the window and the screen on every frame that could move
/// either, carrying only the offset — so there is no state to go stale against a window that
/// changed size under it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Viewport {
    /// The window the daemon arbitrated, in cells, at the origin.
    window: Rect,
    /// The rectangle of this terminal the panes are drawn in — `Split::panes`.
    screen: Rect,
    /// The window cell drawn at [`screen`](Self::screen)'s top-left corner.
    offset: (u16, u16),
}

impl Viewport {
    /// A viewport onto `window` from a terminal whose panes occupy `screen`, showing the window's
    /// top-left corner.
    #[must_use]
    pub const fn of(window: Rect, screen: Rect) -> Self {
        Self::at(window, screen, (0, 0))
    }

    /// A viewport showing `offset` — a position a caller is CARRYING from the last frame, clamped
    /// into what this window and this screen can actually hold.
    ///
    /// The clamp is why a caller may carry it at all. What is remembered is a position in a window
    /// whose size the arbitration can move under it, and an offset past the end of a window that
    /// shrank would leave blank space where the session is; clamping on construction means the
    /// carried value is never a lie, only sometimes older than the person's attention. The
    /// alternative — rebuilding at the origin every frame — is worse and was shipped first: it
    /// throws away the view of any client whose [`follow`](Self::follow) has nothing to follow,
    /// which is a client whose focused pane is FLOATING.
    #[must_use]
    pub const fn at(window: Rect, screen: Rect, offset: (u16, u16)) -> Self {
        let mut view = Self {
            window,
            screen,
            offset,
        };
        let (max_col, max_row) = view.slack();
        if view.offset.0 > max_col {
            view.offset.0 = max_col;
        }
        if view.offset.1 > max_row {
            view.offset.1 = max_row;
        }
        view
    }

    /// How far the viewport can travel on each axis before it runs out of window — what
    /// [`at`](Self::at) and [`follow`](Self::follow) clamp to, and what [`note`](Self::note) reads
    /// to decide there is nothing to say.
    #[must_use]
    const fn slack(&self) -> (u16, u16) {
        (
            self.window.cols.saturating_sub(self.screen.cols),
            self.window.rows.saturating_sub(self.screen.rows),
        )
    }

    /// Whether this terminal is showing the WHOLE window — every ordinary client, and the case in
    /// which everything here is the identity.
    ///
    /// Private, and that is the point rather than an accident: [`note`](Self::note) is the public
    /// answer to this question, and a second public spelling of it would be a second thing a caller
    /// could read and disagree with.
    #[must_use]
    const fn is_whole(&self) -> bool {
        matches!(self.slack(), (0, 0))
    }

    /// The window cell this terminal's top-left pane cell shows — what a caller CARRIES to the next
    /// frame and hands back to [`at`](Self::at).
    #[must_use]
    pub const fn offset(&self) -> (u16, u16) {
        self.offset
    }

    /// Move the view the LEAST it can to put window cell `at` on screen.
    ///
    /// The least, rather than centring it, because the person is reading: a view that jumps to
    /// re-centre on every step of a cursor moving down a file makes the text move under them, where
    /// a view that moves only when the cursor would leave it holds still. It is the rule every
    /// terminal editor scrolls by, arrived at the same way.
    ///
    /// Clamped to the slack between the window and this screen, so a cell in the window's last
    /// column never leaves blank space past the window's edge.
    pub const fn follow(&mut self, at: (u16, u16)) {
        let (max_col, max_row) = self.slack();
        if at.0 < self.offset.0 {
            self.offset.0 = at.0;
        } else if self.screen.cols > 0 && at.0 >= self.offset.0 + self.screen.cols {
            self.offset.0 = at.0 + 1 - self.screen.cols;
        }
        if at.1 < self.offset.1 {
            self.offset.1 = at.1;
        } else if self.screen.rows > 0 && at.1 >= self.offset.1 + self.screen.rows {
            self.offset.1 = at.1 + 1 - self.screen.rows;
        }
        if self.offset.0 > max_col {
            self.offset.0 = max_col;
        }
        if self.offset.1 > max_row {
            self.offset.1 = max_row;
        }
    }

    /// Move the view the LEAST it can to show as much of `rect` as fits, PREFERRING its start.
    ///
    /// [`follow`](Self::follow) for a rectangle rather than a cell, and the two are not the same
    /// request. Following a cell is right when the person's attention is AT that cell — a cursor
    /// in a pane drawn at its own width. It is wrong for a pane this client is RE-WRAPPING, where
    /// the cursor's column in the pane's coordinates is not where the character is drawn: the view
    /// would chase a column the person is not looking at and slide the pane's own left edge off
    /// the screen, which is the one column a re-wrapped pane always starts from.
    ///
    /// "As much as fits, preferring the start" is the far edge followed first and the near edge
    /// second, so the near one wins whenever both cannot be had. For a pane wider than the screen
    /// that pins its first column; for one that fits, it pulls the whole pane on rather than
    /// leaving a tail off the far side.
    pub const fn follow_rect(&mut self, rect: Rect) {
        self.follow((
            rect.col + rect.cols.saturating_sub(1),
            rect.row + rect.rows.saturating_sub(1),
        ));
        self.follow((rect.col, rect.row));
    }

    /// Where a WINDOW rectangle goes on this terminal, or `None` when none of it is on screen.
    ///
    /// This is the one translation. A rectangle the view has scrolled past on either axis comes back
    /// with a [`from`](PaneView::from) saying how much of the pane's own content that cost, and one
    /// that runs off the far edge comes back shortened — so what a caller paints is exactly what
    /// fits, with no reliance on a surface silently dropping writes it cannot place.
    #[must_use]
    pub fn show(&self, area: Rect) -> Option<PaneView> {
        // How many of the rectangle's OWN cells the view has scrolled past.
        let from = (
            self.offset.0.saturating_sub(area.col),
            self.offset.1.saturating_sub(area.row),
        );
        if from.0 >= area.cols || from.1 >= area.rows {
            return None; // entirely left of, or above, the view
        }
        // Where what is left of it starts, in screen coordinates.
        let col = area.col.saturating_sub(self.offset.0);
        let row = area.row.saturating_sub(self.offset.1);
        if col >= self.screen.cols || row >= self.screen.rows {
            return None; // entirely right of, or below, the view
        }
        Some(PaneView {
            area: Rect::new(
                self.screen.col + col,
                self.screen.row + row,
                (area.cols - from.0).min(self.screen.cols - col),
                (area.rows - from.1).min(self.screen.rows - row),
            ),
            from,
        })
    }

    /// The WINDOW cell a screen cell is — [`show`](Self::show) run backwards, for a pointer.
    ///
    /// A mouse report arrives in this terminal's coordinates and every question a client asks of it
    /// (which pane, which divider, what ratio) is asked of a tiling laid out in the WINDOW's. With
    /// the view at the origin the two are the same, which is why nothing needed this until a client
    /// could be smaller than the window it watches.
    #[must_use]
    pub const fn window_cell(&self, col: u16, row: u16) -> (u16, u16) {
        (
            col.saturating_sub(self.screen.col) + self.offset.0,
            row.saturating_sub(self.screen.row) + self.offset.1,
        )
    }

    /// What this client is showing, for the status row — `None` while it shows the whole window.
    ///
    /// It leads the row rather than trailing it, and the reason is the shape of the problem: the
    /// client that owes this sentence is by definition the NARROW one, so a note appended after the
    /// session and its windows is the note that gets truncated away on exactly the terminal it is
    /// for.
    #[must_use]
    pub fn note(&self) -> Option<String> {
        if self.is_whole() {
            return None;
        }
        let (col, row) = self.offset;
        let at = if (col, row) == (0, 0) {
            String::new()
        } else {
            format!(" +{col}+{row}")
        };
        Some(format!(
            "<{}x{} of {}x{}{at}>",
            self.screen.cols, self.screen.rows, self.window.cols, self.window.rows,
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A 60x9 view onto an 80x23 window — the measured case, in the units the paint loop uses.
    fn narrow() -> Viewport {
        Viewport::of(Rect::screen(80, 23), Rect::screen(60, 9))
    }

    /// A client big enough for the window changes NOTHING — the property every existing gate in this
    /// crate rests on, asserted rather than assumed.
    #[test]
    fn a_client_that_fits_the_window_is_the_identity() {
        let whole = Viewport::of(Rect::screen(80, 23), Rect::screen(80, 23));
        assert!(whole.is_whole());
        assert_eq!(whole.note(), None, "a whole view says nothing");
        let area = Rect::new(10, 4, 20, 6);
        assert_eq!(
            whole.show(area),
            Some(PaneView { area, from: (0, 0) }),
            "the rectangle is where the tiling put it, read from its own first cell",
        );
        assert_eq!(whole.window_cell(10, 4), (10, 4));

        // ...and a view that cannot move does not move.
        let mut whole = whole;
        whole.follow((79, 22));
        assert_eq!(whole.offset(), (0, 0));
    }

    /// A terminal LARGER than the window is also whole — there is slack in the other direction and
    /// it is not the viewport's to spend.
    #[test]
    fn a_terminal_larger_than_the_window_is_still_whole() {
        let big = Viewport::of(Rect::screen(40, 10), Rect::screen(80, 24));
        assert!(big.is_whole());
        assert_eq!(big.note(), None);
    }

    /// The measured case: a pane below the view is not shown, and one straddling its edge is shown
    /// as far as it goes.
    #[test]
    fn a_rectangle_outside_the_view_is_not_shown_and_one_across_its_edge_is_cut() {
        let view = narrow();
        assert_eq!(view.show(Rect::new(0, 12, 80, 11)), None, "below the view");
        assert_eq!(view.show(Rect::new(60, 0, 20, 23)), None, "right of it");
        assert_eq!(
            view.show(Rect::new(0, 0, 80, 11)),
            Some(PaneView {
                area: Rect::new(0, 0, 60, 9),
                from: (0, 0),
            }),
            "a pane bigger than the view is shown as far as the view goes",
        );
    }

    /// A rectangle the view has scrolled ENTIRELY past — left of it, or above it — is not shown.
    ///
    /// The other two arms of `show`'s refusal, and they are not symmetrical with the two below: a
    /// rectangle off the FAR edge is refused by its screen position, and one off the NEAR edge by
    /// having no cells of its own left. Neither had a test until the debt sweep asked which
    /// branches nothing builds.
    #[test]
    fn a_rectangle_the_view_has_scrolled_entirely_past_is_not_shown() {
        let mut view = narrow();
        view.follow((0, 22));
        assert_eq!(view.offset(), (0, 14));
        assert_eq!(
            view.show(Rect::new(0, 0, 80, 11)),
            None,
            "a pane whose last row is four rows above the view",
        );

        let mut sideways = Viewport::of(Rect::screen(200, 9), Rect::screen(60, 9));
        sideways.follow((199, 0));
        assert_eq!(sideways.offset(), (140, 0));
        assert_eq!(
            sideways.show(Rect::new(0, 0, 100, 9)),
            None,
            "and a pane whose last column is forty columns left of it",
        );
    }

    /// A carried offset is CLAMPED into the window it is handed, never taken on trust.
    ///
    /// What makes an offset safe to keep across frames: the arbitration can shrink the window under
    /// a client that is not looking, and an offset past its edge would leave blank space where the
    /// session is. The alternative — rebuilding at the origin every frame — was shipped first and
    /// throws away the view of any client whose follow has nothing to follow.
    #[test]
    fn a_carried_offset_is_clamped_into_the_window_it_is_handed() {
        let kept = Viewport::at(Rect::screen(80, 23), Rect::screen(60, 9), (12, 6));
        assert_eq!(kept.offset(), (12, 6), "inside the slack, kept exactly");

        let shrunk = Viewport::at(Rect::screen(64, 12), Rect::screen(60, 9), (12, 6));
        assert_eq!(
            shrunk.offset(),
            (4, 3),
            "the window shrank under it, so the offset comes back to its edge",
        );

        let gone = Viewport::at(Rect::screen(60, 9), Rect::screen(60, 9), (12, 6));
        assert_eq!(
            gone.offset(),
            (0, 0),
            "a window this terminal now fits has no offset to have",
        );
        assert_eq!(gone.note(), None);
    }

    /// Following a cell moves the view the LEAST it can, and never past the window's edge.
    #[test]
    fn following_a_cell_moves_the_view_the_least_it_can() {
        let mut view = narrow();
        view.follow((0, 0));
        assert_eq!(
            view.offset(),
            (0, 0),
            "a cell already on screen moves nothing"
        );

        // The bottom pane's first cell, in the measured arrangement.
        view.follow((0, 12));
        assert_eq!(
            view.offset(),
            (0, 4),
            "just enough to put row 12 on the last of nine rows",
        );
        view.follow((0, 22));
        assert_eq!(
            view.offset(),
            (0, 14),
            "and it is clamped at the window's last row"
        );
        view.follow((0, 0));
        assert_eq!(view.offset(), (0, 0), "back up, again by the least it can");
    }

    /// A pane the view has scrolled PAST is drawn from its own first visible cell — the half an
    /// area alone cannot say.
    #[test]
    fn a_pane_the_view_scrolled_past_is_read_from_its_own_first_visible_cell() {
        let mut view = narrow();
        view.follow((0, 12));
        assert_eq!(view.offset(), (0, 4));
        assert_eq!(
            view.show(Rect::new(0, 0, 80, 11)),
            Some(PaneView {
                area: Rect::new(0, 0, 60, 7),
                from: (0, 4),
            }),
            "the top pane keeps rows 4..11, drawn from the screen's first row",
        );
        assert_eq!(
            view.show(Rect::new(0, 12, 80, 11)),
            Some(PaneView {
                area: Rect::new(0, 8, 60, 1),
                from: (0, 0),
            }),
            "and the bottom pane's first row is now on the screen's last",
        );
    }

    /// The pointer's translation is `show`'s inverse on the cells `show` places.
    #[test]
    fn a_pointer_is_translated_back_into_the_window() {
        let mut view = narrow();
        view.follow((0, 12));
        assert_eq!(
            view.window_cell(0, 8),
            (0, 12),
            "the bottom pane's first cell"
        );
        assert_eq!(view.window_cell(0, 0), (0, 4));
        // Round-trip through `show` for a pane the view has scrolled past.
        let shown = view.show(Rect::new(0, 0, 80, 11)).expect("shown");
        let (col, row) = view.window_cell(shown.area.col, shown.area.row);
        assert_eq!((col, row), (0, 4), "the cell the pane's `from` names");
    }

    /// The note says what is shown and, once the view has moved, where — and it is `None` for every
    /// client that can see the whole window.
    #[test]
    fn the_note_says_what_is_shown_and_where() {
        let mut view = narrow();
        assert_eq!(view.note().as_deref(), Some("<60x9 of 80x23>"));
        view.follow((0, 12));
        assert_eq!(view.note().as_deref(), Some("<60x9 of 80x23 +0+4>"));
    }

    /// **Following a RECTANGLE shows as much of it as fits and prefers its START** — the target a
    /// re-wrapped pane is followed by, where a cursor's column would be meaningless.
    ///
    /// Both arms matter and they are not the same: a pane WIDER than the screen pins its first
    /// column (there is no offset that shows all of it, so the one that shows its beginning wins),
    /// and one that FITS is pulled on whole rather than left with a tail off the far side.
    ///
    /// REVERT-PROOF: follow only the near edge and the second case never moves — a pane at column
    /// 50 keeps ten of its fifty columns forever.
    #[test]
    fn following_a_rectangle_shows_as_much_of_it_as_fits_and_prefers_its_start() {
        let mut whole = Viewport::of(Rect::screen(100, 23), Rect::screen(60, 23));
        whole.follow_rect(Rect::new(0, 0, 100, 23));
        assert_eq!(
            whole.offset(),
            (0, 0),
            "a pane wider than the screen is shown from its own first column",
        );

        let mut right = Viewport::of(Rect::screen(100, 23), Rect::screen(60, 23));
        right.follow_rect(Rect::new(50, 0, 50, 23));
        assert_eq!(
            right.offset(),
            (40, 0),
            "and a pane that FITS is pulled on whole — all fifty of its columns, not ten",
        );

        // Which is the case that needs the far edge first: following the near one alone leaves the
        // view where it was, because column 50 is already on a screen showing 0..60.
        let mut near_only = Viewport::of(Rect::screen(100, 23), Rect::screen(60, 23));
        near_only.follow((50, 0));
        assert_eq!(near_only.offset(), (0, 0), "the control for the line above");
    }

    /// **A RE-WRAPPED PANE STARTS AT ITS OWN FIRST ROW AND MOVES DOWN BY THE LEAST THAT KEEPS THE
    /// CURSOR ON SCREEN** — and the measured case is the first one.
    ///
    /// A 100-column pane holding one wrapped line and twenty-two blank rows re-wraps to
    /// twenty-four rows for a twenty-three-row client. Bottom-anchoring was written first and
    /// driven through the shipped binary, where it dropped the row over the limit: the first HALF
    /// of the only line on the screen, to make room for a blank one.
    ///
    /// REVERT-PROOF: anchor at the bottom (`tall - on_screen`) and the first case answers 1.
    #[test]
    fn a_re_wrapped_pane_shows_its_first_row_until_the_cursor_needs_it_not_to() {
        assert_eq!(
            first_row(24, 23, Some(1)),
            0,
            "the measured case: one wrapped line and a blank row over the limit",
        );
        assert_eq!(first_row(23, 23, Some(22)), 0, "content that exactly fits");
        assert_eq!(
            first_row(10, 23, Some(3)),
            0,
            "content shorter than the view"
        );
        assert_eq!(
            first_row(46, 23, Some(44)),
            22,
            "a cursor past the bottom scrolls by the LEAST that puts it on the last row",
        );
        assert_eq!(
            first_row(46, 23, Some(45)),
            23,
            "...and never past what the content can fill",
        );
        assert_eq!(
            first_row(46, 23, None),
            23,
            "a frame with NO cursor — a scrolled-back window — is anchored at its bottom",
        );
        assert_eq!(first_row(0, 0, None), 0, "and nothing divides by nothing");
    }

    /// A screen with no room at all cannot be divided by zero.
    #[test]
    fn a_screen_of_nothing_shows_nothing_and_does_not_panic() {
        let mut view = Viewport::of(Rect::screen(80, 23), Rect::screen(0, 0));
        view.follow((40, 12));
        assert_eq!(view.show(Rect::new(0, 0, 80, 23)), None);
        assert!(
            !view.is_whole(),
            "a screen showing nothing is not showing it all"
        );
    }
}
