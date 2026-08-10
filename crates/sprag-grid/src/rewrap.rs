//! The client-side re-wrap — see [`rewrap`] for what it is and why it exists.
//!
//! PRIVATE, and the three items are re-exported at the crate root. The module was public once and
//! the doc gate caught what that cost: `sprag_grid::rewrap` then named a module AND a function, and
//! a word that names two things is a defect waiting (R344). The narrative moved onto the function,
//! which is where a reader looks for it anyway.

use pinion_core::{CellWidth, GridBuffer, Hyperlink, HyperlinkId, ScreenKind, TermCell};
use sprag_vt::Screen;

use crate::projected_rows;

/// Where each projected row's share of its logical line ENDS, and which rows run on — the fact a
/// re-wrap needs and a rectangle of cells cannot carry.
///
/// Positional, like the damage stamps beside it: [`upto`](Self::upto) has one entry per projected
/// row, in row order. [`continues`](Self::continues) is SPARSE — the rows whose line runs onto the
/// next — because nearly every screen has none, and a fact that costs nothing when it says nothing
/// is one nobody has to decide whether to send.
///
/// The two are built together by [`shares`] from one walk of one screen, so they cannot describe
/// different frames.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct RowShares {
    /// How many of each row's cells belong to its logical line — `upto[r]` cells of row `r`, and
    /// the columns from there are LAYOUT rather than text: the pad a wide cluster leaves when it
    /// will not fit at the margin, or the grid's own padding past the end of a line.
    #[serde(default)]
    pub upto: Vec<u16>,
    /// The rows whose logical line CONTINUES onto the next one, ascending. Empty for a screen on
    /// which nothing soft-wrapped, which is the ordinary one.
    #[serde(default)]
    pub continues: Vec<u16>,
}

impl RowShares {
    /// Whether this says nothing at all — a frame nobody derived shares for.
    ///
    /// The wire's `skip_serializing_if`, so a host that has not been asked for the fact costs a
    /// reader nothing, and a reader that gets nothing draws the pane as it stands.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.upto.is_empty() && self.continues.is_empty()
    }

    /// Whether this describes a buffer of `rows` rows — one share per row, which is the ONLY
    /// shape a re-wrap may act on.
    ///
    /// The guard exists because the alternative was a live defect. Empty shares are what a host
    /// answers when it cannot say (the trait default, a daemon older than the fact, an absent
    /// pane), and falling back to "the whole row belongs to the line" turns a pane's blank padding
    /// into text: a 100-column pane became twice as tall at sixty columns, blank rows and all.
    #[must_use]
    fn describes(&self, rows: u16) -> bool {
        self.upto.len() == usize::from(rows)
    }

    /// How many of row `row`'s cells belong to its logical line.
    ///
    /// Total, because [`Self::describes`] has already been checked by the only caller — a share
    /// missing here would be a vector that changed length between the two, which it cannot.
    #[must_use]
    fn upto_of(&self, row: u16) -> u16 {
        self.upto.get(usize::from(row)).copied().unwrap_or(0)
    }

    /// Whether row `row`'s logical line runs onto row `row + 1`.
    #[must_use]
    fn continues_at(&self, row: u16) -> bool {
        self.continues.binary_search(&row).is_ok()
    }
}

/// Read [`RowShares`] for the same rows [`project_scrolled`](crate::project_scrolled) would
/// project at `offset_lines` — the ONE derivation, beside the projection whose rows it describes.
///
/// It walks the crate's own `projected_rows`, the same enumerator the projection walks, so a row's
/// share can never be read off a different row than its cells were: the two would have to disagree
/// about which screen row a display row IS, and there is only one place that decides.
#[must_use]
pub fn shares(screen: &Screen, offset_lines: usize) -> RowShares {
    let mut upto = Vec::with_capacity(usize::from(screen.rows()));
    let mut continues = Vec::new();
    for (display, at) in projected_rows(screen, offset_lines).enumerate() {
        let row = u16::try_from(display).unwrap_or(u16::MAX);
        let (share, runs_on) = at.share(screen);
        upto.push(share);
        if runs_on {
            continues.push(row);
        }
    }
    RowShares { upto, continues }
}

///  Re-wrap a projected pane into a NARROWER display — the client-side half of a shared pane.
///
///  ## The measurement this exists for
///
///  A window is arbitrated across every client watching it, so a client's terminal can be narrower
///  than the pane it is looking at. R346 settled that this can never be a re-layout (a pty has ONE
///  winsize) and R348 gave the client a VIEWPORT that follows the cursor of the pane being typed
///  into. What that left, driven on a 60-column client watching a 100-column window with one
///  78-character line on it:
///
///  * **while typing** the view sat at `+19`, so the screen read `----…----END` — the first
///    nineteen columns of the person's own line were not on it, and no key moves a view that is
///    pinned to the cursor;
///  * **after Enter** the cursor returned to column 0, the view snapped back to `+0`, and the same
///    line read `START----…` — now the END is the part that cannot be reached.
///
///  So a 60-column client can read sixty columns of a hundred-column line, and WHICH sixty is
///  decided by where the cursor happens to be. Twenty-two characters of a committed line are
///  unreadable for good.
///
///  ## What this does instead
///
///  [`rewrap`] rebuilds the pane's LOGICAL lines and cuts them to the width the client can actually
///  show, so the whole line is on screen at once and there is nothing left to pan to. It is the
///  rule the emulator itself uses when a terminal is narrowed ([`sprag_vt::Screen::reflowed`]) —
///  applied to ONE client's picture rather than to the pane, so the pty's winsize, the other
///  clients, and the child's idea of its own width are all untouched.
///
///  ## Why it needs a fact the cells cannot carry
///
///  A [`GridBuffer`] is a rectangle of cells and says nothing about where one logical line ends and
///  the next begins — the producer knows, and R344 recorded what happens when a reader guesses:
///  three of them did, and the guess injected a space into the user's text. So the producer sends
///  it. [`RowShares`] is that fact, derived once by [`shares`] through the same
///  [`sprag_vt::Screen::continues`] the reflow, the durable history and the search read, and there
///  is no second place where "how much of this row belongs to its line" is decided.
///
///  ## Where it must NOT run
///
///  A program on the ALTERNATE screen owns absolute cell positions at the width it was told: its
///  rows are a layout, not lines, and re-wrapping them is corruption rather than a reshape. A pane
///  crosses that line whenever somebody opens `vim`, so the refusal is [`rewrap`]'s own and not a
///  caller's to remember.
///
///
///
/// # The contract
///
/// Re-wrap `buffer`'s logical lines into `cols` columns, or `None` when it must be drawn as it is.
///
/// `None` has one meaning for a caller — **draw this pane the way you already do** — and three
/// causes, each of which is a reason nothing here should run rather than a failure:
///
/// * the buffer is the ALTERNATE screen, whose rows are a layout at a width the program was told;
/// * `cols` is zero, so there is no width to wrap into;
/// * the buffer is already no wider than `cols`, so its lines are the client's lines already;
/// * `shares` does not describe THESE rows — one entry per row or nothing. Empty is what a host
///   answers when it cannot say, and cutting a pane at the grid's width instead of at its lines'
///   ends is not a fallback, it is a different picture: blank padding becomes text and the pane
///   comes back twice as tall.
///
/// The result is a buffer of `cols` columns and as many rows as the content needs — usually MORE
/// than the pane has, which is the point: a caller shows the part of it the cursor is in, exactly
/// as a terminal shows the bottom of its own scrollback. The cursor is carried across to where its
/// cell ended up, so a client painting from this puts it under the character the person is typing.
///
/// A wide cluster is never split across the cut: it moves to the next row whole and the row it
/// left is padded, which is what a terminal does at its own margin. The one exception is a view
/// one column wide, where a wide cluster cannot be kept whole by any cut — there the head is
/// emitted alone rather than the loop failing to advance.
#[must_use]
pub fn rewrap(buffer: &GridBuffer, shares: &RowShares, cols: u16) -> Option<GridBuffer> {
    if buffer.screen() == ScreenKind::Alternate
        || cols == 0
        || buffer.cols() <= cols
        || !shares.describes(buffer.rows())
    {
        return None;
    }
    let cursor = buffer.cursor();
    let mut rows: Vec<Vec<TermCell>> = Vec::new();
    let mut cursor_at = (cursor.col, cursor.row);
    // ONE join buffer for the whole pass, reserved to each line's exact length before it is
    // filled. Measured: growing a fresh vector per line costs one extra reallocation per line for
    // every doubling of the width, which put a re-wrap's cost on the CELLS after all — 223
    // allocations against 247 for the same rows at twice the width. Reused and reserved, both are
    // the same number, and `rewrap_allocs.rs` is what says so.
    let mut line: Vec<TermCell> = Vec::new();

    let mut row = 0;
    while row < buffer.rows() {
        let span = line_rows(shares, buffer.rows(), row);
        let need: usize = span
            .clone()
            .map(|at| usize::from(shares.upto_of(at).min(buffer.cols())))
            .sum();
        line.clear();
        line.reserve(need);
        // Where the CURSOR's cell landed in the joined line, `None` until its row is reached.
        let mut at: Option<usize> = None;
        for on in span.clone() {
            let share = shares.upto_of(on).min(buffer.cols());
            if cursor.row == on {
                at = Some(line.len() + usize::from(cursor.col.min(share)));
            }
            for col in 0..share {
                line.push(
                    buffer
                        .cell(col, on)
                        .cloned()
                        .unwrap_or_else(TermCell::blank),
                );
            }
        }
        let first = rows.len();
        cut(&line, cols, &mut rows);
        if let Some(at) = at {
            cursor_at = landing(&rows[first..], first, at, cols);
        }
        row = *span.end() + 1;
    }

    let mut out = GridBuffer::new(cols, u16::try_from(rows.len()).unwrap_or(u16::MAX));
    for (row, cells) in rows.into_iter().enumerate() {
        let Ok(row) = u16::try_from(row) else { break };
        if row >= out.rows() {
            break;
        }
        out = out.with_row(row, cells);
    }
    // `GridCursor` is `#[non_exhaustive]`, so the carried cursor is the ORIGINAL with its two
    // coordinates moved — which is what a caller wants anyway: shape, visibility and OSC-12 colour
    // are the producer's and have nothing to do with where the cell ended up.
    let mut moved = cursor;
    moved.col = cursor_at.0;
    moved.row = cursor_at.1;
    Some(
        out.with_screen(buffer.screen())
            .with_hyperlinks(link_table(buffer))
            .with_cursor(moved),
    )
}

/// The rows one logical line occupies, starting at `row` — the ONE decision about where a line
/// ends, and it is a function because there are two consumers of the answer.
///
/// The join needs it to walk the cells, and the reservation needs it to know how many there will
/// be. Two walks that each decided for themselves would be R344's defect in miniature: a line
/// joined over three rows and sized for two reallocates in the middle of the copy, and one sized
/// for four reserves for text that is not there.
fn line_rows(shares: &RowShares, rows: u16, row: u16) -> std::ops::RangeInclusive<u16> {
    let mut last = row;
    while shares.continues_at(last) && last + 1 < rows {
        last += 1;
    }
    row..=last
}

/// Cut one logical line into rows of `cols`, padding each to the full width.
///
/// An EMPTY line still produces one row, and that is the whole reason this appends rather than
/// returning: a blank row on the pane is a blank row on the client, and a line that vanished
/// because it had no cells would slide every row below it up by one.
fn cut(line: &[TermCell], cols: u16, rows: &mut Vec<Vec<TermCell>>) {
    let width = usize::from(cols);
    let mut at = 0;
    loop {
        let mut end = (at + width).min(line.len());
        // Never split a wide cluster from its trailer: the head goes to the next row whole and
        // this one is padded, the substitution a terminal makes at its own margin.
        if end < line.len() && line[end].width == CellWidth::Trailer {
            end -= 1;
        }
        if end == at && at < line.len() {
            // A one-column view onto a wide cluster: no cut keeps it whole, so the head is emitted
            // alone rather than the loop failing to advance.
            end = at + 1;
        }
        let mut row: Vec<TermCell> = line[at..end].to_vec();
        row.resize(width, TermCell::blank());
        rows.push(row);
        at = end;
        if at >= line.len() {
            return;
        }
    }
}

/// Where cell `at` of a line whose rows start at `first` ended up, in the output's coordinates.
///
/// Walks the rows the cut produced rather than dividing by `cols`, because the cut does not always
/// divide evenly: a row that gave a wide cluster up to the next one is a column short, and an
/// index mapped by arithmetic would land one cell to the right of the character it names for every
/// row after it.
fn landing(rows: &[Vec<TermCell>], first: usize, at: usize, cols: u16) -> (u16, u16) {
    let mut seen = 0;
    for (index, row) in rows.iter().enumerate() {
        // A row's own cells, not its padded width: the padding is not part of the line.
        let held = row
            .iter()
            .rposition(|cell| cell.cluster != " ")
            .map_or(0, |last| last + 1);
        let held = held.max(1).min(usize::from(cols));
        if at < seen + held || index + 1 == rows.len() {
            let col = u16::try_from(at - seen)
                .unwrap_or(0)
                .min(cols.saturating_sub(1));
            return (col, u16::try_from(first + index).unwrap_or(u16::MAX));
        }
        seen += held;
    }
    (0, u16::try_from(first).unwrap_or(u16::MAX))
}

/// A copy of `buffer`'s OSC-8 interning table, so a re-wrapped cell's [`HyperlinkId`] still
/// resolves to the link it was interned for.
///
/// Read by walking ids until one does not resolve, because pinion exposes the table one entry at a
/// time ([`GridBuffer::hyperlink`]) and offers no accessor for the whole of it. Re-interning only
/// the ids the cells still use would renumber them, which would break the one thing an id is for:
/// tying the halves of a link SPLIT ACROSS A WRAP into one logical link — the very split this
/// module makes more of.
fn link_table(buffer: &GridBuffer) -> Vec<Hyperlink> {
    let mut table = Vec::new();
    while let Some(link) = u32::try_from(table.len())
        .ok()
        .and_then(|id| buffer.hyperlink(HyperlinkId(id)))
    {
        table.push(link.clone());
    }
    table
}

#[cfg(test)]
mod tests {
    use super::*;
    use pinion_core::TermColor;
    use sprag_vt::{Emulator, Palette, VtPort};

    /// A screen a child has printed `bytes` onto, at `cols` x `rows`.
    fn screen_of(bytes: &[u8], cols: u16, rows: u16) -> sprag_vt::Screen {
        let mut em = Emulator::new(cols, rows);
        em.advance(bytes);
        em.screen().clone()
    }

    /// A projected buffer plus the shares that describe it — the pair every caller holds, taken
    /// from ONE screen so a test cannot pair them wrongly by accident.
    fn frame_of(bytes: &[u8], cols: u16, rows: u16) -> (GridBuffer, RowShares) {
        let screen = screen_of(bytes, cols, rows);
        let palette = Palette::xterm_default();
        (crate::project(&screen, &palette), shares(&screen, 0))
    }

    /// A re-wrapped buffer's rows as text, trailing blanks trimmed — what a person would read.
    fn rows_of(buffer: &GridBuffer) -> Vec<String> {
        (0..buffer.rows())
            .map(|row| {
                (0..buffer.cols())
                    .map(|col| {
                        buffer
                            .cell(col, row)
                            .map_or(" ", |cell| cell.cluster.as_ref())
                    })
                    .collect::<String>()
                    .trim_end()
                    .to_owned()
            })
            .collect()
    }

    /// **THE MEASURED CASE.** A 78-character line on a 100-column pane, re-wrapped for a
    /// 60-column client: both ends are on screen at once, which is exactly what the driven probe
    /// said no view of the un-wrapped pane can do.
    ///
    /// REVERT-PROOF: return the buffer unchanged and row 0 holds the first sixty columns with
    /// `END` nowhere, which is the defect.
    #[test]
    fn a_long_line_comes_back_as_the_rows_a_narrow_client_can_show() {
        let line = format!("START{}END", "-".repeat(70));
        let (buffer, shares) = frame_of(line.as_bytes(), 100, 5);
        let narrow = rewrap(&buffer, &shares, 60).expect("a 100-column pane re-wraps into 60");

        assert_eq!(narrow.cols(), 60);
        let rows = rows_of(&narrow);
        assert_eq!(
            rows[0],
            format!("START{}", "-".repeat(55)),
            "the line's first sixty columns",
        );
        assert_eq!(
            rows[1],
            format!("{}END", "-".repeat(15)),
            "...and the rest of it on the row below, where the un-wrapped view could not reach",
        );
        assert_eq!(
            rows[0].chars().count() + rows[1].chars().count(),
            line.chars().count(),
            "every character of the line is on screen and none of it twice",
        );
    }

    /// A line the pane ITSELF wrapped is joined before it is cut — the fact the cells cannot carry.
    ///
    /// 150 characters on a 100-column pane is two pane rows; on a 60-column client it must be
    /// three, not "row 0 cut in two, then row 1 cut in two".
    ///
    /// REVERT-PROOF: stop consuming `continues` (treat every row as its own line) and the answer
    /// is four rows, with the join falling at column 100 instead of running straight on.
    #[test]
    fn a_line_the_pane_already_wrapped_is_rejoined_before_it_is_cut() {
        let line: String = (0..150)
            .map(|i| char::from(b'a' + (i % 26) as u8))
            .collect();
        let (buffer, shares) = frame_of(line.as_bytes(), 100, 5);
        assert_eq!(
            shares.continues,
            vec![0],
            "the pane wrapped row 0 onto row 1"
        );

        let narrow = rewrap(&buffer, &shares, 60).expect("re-wraps");
        let rows = rows_of(&narrow);
        assert_eq!(
            &rows[..3],
            &[
                line[..60].to_owned(),
                line[60..120].to_owned(),
                line[120..].to_owned(),
            ],
            "150 characters is three rows of sixty, joined across the pane's own wrap",
        );
    }

    /// **THE ALTERNATE SCREEN IS REFUSED, and by this function rather than by its caller.**
    ///
    /// A program there owns absolute cell positions at the width it was told, so its rows are a
    /// layout and re-wrapping them is corruption. A pane crosses that line whenever somebody opens
    /// `vim`, so a rule a caller has to remember is a rule that gets forgotten.
    ///
    /// REVERT-PROOF: drop the `ScreenKind::Alternate` arm and this returns a buffer.
    #[test]
    fn the_alternate_screen_is_never_re_wrapped() {
        // Switch to the alternate screen (DEC 1049) and print a line wider than the target.
        let mut bytes = b"\x1b[?1049h".to_vec();
        bytes.extend(std::iter::repeat_n(b'x', 80));
        let (buffer, shares) = frame_of(&bytes, 100, 5);
        assert_eq!(buffer.screen(), ScreenKind::Alternate);
        assert!(
            rewrap(&buffer, &shares, 60).is_none(),
            "a fullscreen program's layout is not a set of lines",
        );
    }

    /// The two other refusals, which are the same answer for a caller: nothing to wrap into, and
    /// nothing that needs wrapping.
    #[test]
    fn a_pane_that_already_fits_and_a_view_of_no_width_are_both_left_alone() {
        let (buffer, shares) = frame_of(b"hello", 60, 5);
        assert!(rewrap(&buffer, &shares, 60).is_none(), "already this wide");
        assert!(
            rewrap(&buffer, &shares, 80).is_none(),
            "wider than the pane"
        );
        assert!(
            rewrap(&buffer, &shares, 0).is_none(),
            "no width to wrap into"
        );
    }

    /// **A BLANK ROW IS STILL A ROW.** A line with no cells produces one output row, so the rows
    /// below it do not slide up — the pane's arrangement of blank space is content too.
    ///
    /// REVERT-PROOF: skip an empty line instead of emitting its row and the second line lands on
    /// row 0, one row above where the pane put it.
    #[test]
    fn an_empty_line_keeps_its_own_row() {
        let (buffer, shares) = frame_of(b"one\r\n\r\nthree", 100, 5);
        let narrow = rewrap(&buffer, &shares, 60).expect("re-wraps");
        let rows = rows_of(&narrow);
        assert_eq!(&rows[..3], &["one", "", "three"], "the blank row survives");
        assert_eq!(
            narrow.rows(),
            5,
            "and a pane whose lines all fit keeps exactly its own row count",
        );
    }

    /// The CURSOR is carried to the cell it ended up in, which is what makes a client painting
    /// from this put it under the character the person is typing.
    ///
    /// REVERT-PROOF: leave the original cursor on the buffer and it reports column 78 of a
    /// 60-column buffer — off the screen it is describing.
    #[test]
    fn the_cursor_moves_to_where_its_cell_landed() {
        let line = format!("START{}END", "-".repeat(70));
        let (buffer, shares) = frame_of(line.as_bytes(), 100, 5);
        assert_eq!(
            (buffer.cursor().col, buffer.cursor().row),
            (78, 0),
            "the pane has it one past the line's last cell",
        );

        let narrow = rewrap(&buffer, &shares, 60).expect("re-wraps");
        let moved = narrow.cursor();
        assert_eq!(
            (moved.col, moved.row),
            (18, 1),
            "which is column 18 of the second row once the line is cut at sixty",
        );
        assert_eq!(
            moved.visible,
            buffer.cursor().visible,
            "and nothing else about the cursor is this function's business",
        );
    }

    /// A wide cluster is never split across the cut — it moves to the next row whole, which is the
    /// substitution a terminal makes at its own margin.
    ///
    /// REVERT-PROOF: drop the `Trailer` check and the head sits in the last column with its
    /// trailer at the start of the next row, which paints a half-glyph on two rows.
    #[test]
    fn a_wide_cluster_moves_to_the_next_row_rather_than_being_split() {
        // Nine narrow columns then a wide cluster, cut at ten: the cluster cannot fit.
        let mut bytes = b"abcdefghi".to_vec();
        bytes.extend("\u{d55c}".as_bytes());
        bytes.extend(b"jkl");
        let (buffer, shares) = frame_of(&bytes, 20, 3);
        let narrow = rewrap(&buffer, &shares, 10).expect("re-wraps");
        let rows = rows_of(&narrow);
        assert_eq!(rows[0], "abcdefghi", "the ninth column is left blank");
        assert!(
            rows[1].starts_with('\u{d55c}'),
            "and the cluster begins the next row whole: {rows:?}",
        );
    }

    /// A one-column view cannot keep a wide cluster whole by any cut, so the head is emitted alone
    /// and the walk still terminates. The degenerate arm, named because a loop that failed to
    /// advance here would hang a client rather than paint it badly.
    #[test]
    fn a_view_one_column_wide_terminates_on_a_wide_cluster() {
        let (buffer, shares) = frame_of("\u{d55c}\u{ae00}".as_bytes(), 20, 2);
        let narrow = rewrap(&buffer, &shares, 1).expect("re-wraps");
        assert!(narrow.rows() >= 4, "one row per column: {}", narrow.rows());
    }

    /// **A TRAILING RUN CARRYING A BACKGROUND COLOUR IS CONTENT, NOT PADDING** — the distinction
    /// `line_cells` records as having cost a defect, arriving here through the share rather than
    /// being re-decided.
    ///
    /// The colour is compared against the WIDE buffer's own cell rather than against a named
    /// value, because the projection resolves a palette index to an RGB triple and this test is
    /// about the cell surviving, not about what red is.
    ///
    /// REVERT-PROOF: compute the share by trimming space CLUSTERS instead of whole cells and the
    /// coloured bar is dropped, so a person watching on a narrow client loses a stripe the wide
    /// client shows.
    #[test]
    fn a_coloured_trailing_run_is_part_of_the_line() {
        // Ten characters, then a red-background run of twenty spaces, on a 40-column pane.
        let mut bytes = b"0123456789\x1b[41m".to_vec();
        bytes.extend(std::iter::repeat_n(b' ', 20));
        bytes.extend(b"\x1b[0m");
        let (buffer, shares) = frame_of(&bytes, 40, 3);
        assert_eq!(shares.upto[0], 30, "the coloured run is in the line");
        let bar = buffer.cell(20, 0).expect("a cell inside the run").bg;
        assert_ne!(bar, TermColor::Default, "the run must be visibly coloured");

        let narrow = rewrap(&buffer, &shares, 20).expect("re-wraps");
        assert_eq!(
            narrow.cell(0, 1).map(|cell| cell.bg),
            Some(bar),
            "the second row starts inside the coloured bar",
        );
        assert_eq!(
            narrow.cell(9, 1).map(|cell| cell.bg),
            Some(bar),
            "...and the bar runs to the line's last cell",
        );
        assert_eq!(
            narrow.cell(10, 1).map(|cell| cell.bg),
            Some(TermColor::Default),
            "...where the line ends and the row's padding begins",
        );
    }

    /// **A HOST THAT CANNOT SAY WHERE THE LINES END IS NOT RE-WRAPPED FOR** — the arm the debt
    /// sweep found, and it was a live defect rather than a missing test.
    ///
    /// Empty [`RowShares`] is the answer a host gives when it has nothing to say: the trait default
    /// answers it, a daemon that predates the fact answers it, and an absent pane answers it. The
    /// documented reading is "draw the pane as it stands". Nothing enforced that — `upto_of` fell
    /// back to the WHOLE ROW, so every row of a 100-column pane became two rows of sixty including
    /// the blank ones, a 23-row pane became 46, and the client showed the blank half of it.
    ///
    /// The fix is the refusal, not a check at the call site: a caller that has to remember is the
    /// caller R344 and R343 both wrote rules about.
    ///
    /// REVERT-PROOF: drop the `describes` guard and this returns a buffer twice as tall.
    #[test]
    fn a_buffer_whose_shares_are_missing_or_the_wrong_shape_is_left_alone() {
        let (buffer, shares) = frame_of(b"hello", 100, 23);
        assert!(
            rewrap(&buffer, &RowShares::default(), 60).is_none(),
            "a host that said nothing must not have its rows cut at the grid's width",
        );

        // A share vector describing a DIFFERENT screen is the same answer, and it is reachable:
        // the shares and the cells ride together, but a caller can still be handed a pair from a
        // host that changed size between them.
        let stale = RowShares {
            upto: shares.upto[..5].to_vec(),
            continues: Vec::new(),
        };
        assert!(
            rewrap(&buffer, &stale, 60).is_none(),
            "shares that do not describe these rows describe nothing about them",
        );

        // ...and the control: the real pair still re-wraps, so this is about the shares and not
        // about the fixture.
        assert!(rewrap(&buffer, &shares, 60).is_some());
    }

    /// **AN OSC-8 LINK STILL RESOLVES AFTER THE CUT, AND THE HALVES STAY ONE LINK.**
    ///
    /// A cell references its link by an INDEX into the buffer's interning table, so a re-wrap that
    /// dropped or renumbered the table would leave a `ls --hyperlink` line pointing at the wrong
    /// URI or at nothing. The halves keeping the SAME id is the second half of the claim and it is
    /// what an id is for: tying a link split across a wrap into one logical link — the very split
    /// this module makes more of.
    ///
    /// REVERT-PROOF: return an empty table from `link_table` and both resolutions come back
    /// `None`; re-intern only the used ids and the two halves get different indices.
    #[test]
    fn a_hyperlink_survives_the_cut_and_both_halves_stay_one_link() {
        // A 40-column pane with a 30-character link on it, cut at 20.
        let mut bytes = b"\x1b]8;id=one;https://example.test\x1b\\".to_vec();
        bytes.extend(std::iter::repeat_n(b'L', 30));
        bytes.extend(b"\x1b]8;;\x1b\\");
        let (buffer, shares) = frame_of(&bytes, 40, 3);
        assert!(
            buffer.cell_hyperlink(0, 0).is_some(),
            "the fixture must carry a link or this proves nothing",
        );

        let narrow = rewrap(&buffer, &shares, 20).expect("re-wraps");
        let first = narrow.cell_hyperlink(0, 0).expect("the link's first half");
        let second = narrow.cell_hyperlink(0, 1).expect("its second half");
        assert_eq!(first.uri, "https://example.test");
        assert_eq!(
            narrow.cell(0, 0).and_then(|cell| cell.hyperlink),
            narrow.cell(0, 1).and_then(|cell| cell.hyperlink),
            "the two rows are the SAME link, not two that happen to point alike",
        );
        assert_eq!(second.uri, first.uri);
    }

    /// The shares describe the rows the projection actually built, at any scroll offset — the two
    /// walk one enumerator, and this is what says so.
    ///
    /// The fixture pushes a WRAPPED line into history and scrolls back far enough to reach it: at
    /// offset 3 the top projected row is the first half of that line, at offset 2 it is the
    /// second. So a walk that ignored the offset would report the same flags for both, and one
    /// that was off by a row would put the wrap on the wrong one.
    ///
    /// REVERT-PROOF: give `shares` its own row arithmetic (drop the offset) and the flags describe
    /// the LIVE rows while the cells are history, so a re-wrap cuts lines that are not there.
    #[test]
    fn the_shares_describe_the_same_rows_the_projection_scrolled_to() {
        // Thirty columns of one line on a 20-column pane (so it wraps), then three more lines
        // through a 3-row pane, which pushes the wrapped line's halves into history.
        let screen = screen_of(
            b"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\r\ntwo\r\nthree\r\nfour\r\n",
            20,
            3,
        );
        assert_eq!(screen.scrollback_len(), 3, "three rows went into history");
        let palette = Palette::xterm_default();

        let deep = shares(&screen, 3);
        assert_eq!(
            deep.upto.len(),
            usize::from(crate::project_scrolled(&screen, 3, &palette).rows()),
            "one share per projected row",
        );
        assert_eq!(
            deep,
            RowShares {
                upto: vec![20, 10, 3],
                continues: vec![0],
            },
            "at the top of history the wrapped line's first half IS projected row 0",
        );
        assert_eq!(
            shares(&screen, 2),
            RowShares {
                upto: vec![10, 3, 5],
                continues: Vec::new(),
            },
            "one row later it has scrolled past, and nothing left in view runs on",
        );
        assert_eq!(
            shares(&screen, 0),
            RowShares {
                upto: vec![5, 4, 0],
                continues: Vec::new(),
            },
            "and the live view is the rows the child is printing on",
        );
    }
}
