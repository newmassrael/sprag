//! The tabulation stops of a terminal: the columns a horizontal tab lands on, and the rows a
//! vertical one does.
//!
//! # The defect this removes
//!
//! sprag computed a tab as `((col / 8) + 1) * 8` — a FIXED eight-column grid with no table behind
//! it. Every sequence that exists to move that grid was parsed by termwiz and then dropped by the
//! emulator's catch-all arm: HTS (`ESC H`), CTC (`CSI W`), TBC (`CSI g`), CBT (`CSI Z`), CHT
//! (`CSI I`), CVT (`CSI Y`) and DECST8C (`CSI ? 5 W`). An application that set its own stops was
//! not refused, it was ignored — the worst of the three outcomes, because the columns it then
//! printed into were wrong rather than absent.
//!
//! # The model
//!
//! ECMA-48 has two independent stop sets: CHARACTER tab stops (columns, moved by HT / CHT / CBT)
//! and LINE tab stops (rows, moved by CVT). Both are DEVICE-wide — one set shared by every line —
//! which is the model every real terminal implements and the one ECMA-48 itself defaults to
//! (TSM reset). That is why [`TabStops`] holds two [`StopSet`]s and not a per-row table: with
//! device-wide stops, "clear this line's stops" and "clear all stops" name the same set, and a
//! per-row table would make representable a state no sequence can ever address.
//!
//! Power-on state: character stops every eight columns, and NO line stops. A terminal with no
//! line stops answers CVT by moving to the bottom of the region, which is what the sequence means
//! when the set is empty.
//!
//! # Why a stop set outlives a resize
//!
//! [`StopSet::reserve`] GROWS the table and never rebuilds it, so an application's stops survive a
//! resize. This is the deliberate divergence from Ghostty, whose `resize` doc states *"If the
//! column count changes, tabstops are reset"* and rebuilds the table from the eight-column default
//! (`Terminal.zig` at `2602886`).
//!
//! For a single-window terminal that costs little: a resize is a human dragging a window edge, and
//! it is rare. sprag is a MULTIPLEXER, where a resize is routine and mostly not addressed to the
//! application at all — a divider drag, a pane zoom and unzoom, a second client attaching at a
//! different size, a split. Resetting the stops there would let one client's window shape silently
//! destroy another pane's layout state. So the rule is: a resize is a change of GEOMETRY, and a tab
//! stop is state the CHILD set. Only the child (TBC / CTC / DECST8C) and a RIS may move it.
//!
//! The `tail` field is what makes that promise complete in both directions. A set only materialises
//! bits for positions it has been asked about, so a narrow pane that widens has to answer for
//! columns it has never seen. The honest answer is not "eight-column default" — it is whatever the
//! child last said about positions in general: after `CSI 3 g` (clear ALL character tab stops) the
//! new columns arrive with no stops, because "all" included the ones that did not exist yet. And
//! because the table only grows, a pane that narrows and widens again finds its stops where it
//! left them.

use std::num::NonZeroU16;

/// One tabulation stop set: the positions along ONE axis that a tab lands on.
///
/// Positions are 0-based. A set answers for EVERY `u16` position, not only the ones it has
/// materialised — see the module docs for why the unmaterialised tail carries a rule rather than a
/// default.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct StopSet {
    /// One bit per position, least-significant bit first within each word. Positions below
    /// `words.len() * 64` have an explicit answer here; the rest are `tail`'s.
    words: Vec<u64>,
    /// What a position past the materialised range is: `Some(n)` = every `n`-th position is a
    /// stop, `None` = no position is. A `NonZeroU16` because an interval of zero would describe a
    /// stop at every position and at none of them at once.
    tail: Option<NonZeroU16>,
}

/// The bits one word of [`StopSet::words`] covers.
const BITS: u16 = 64;

impl StopSet {
    /// A set with a stop every `interval` positions, materialised far enough to answer for
    /// `positions` of them.
    #[must_use]
    pub fn every(interval: NonZeroU16, positions: u16) -> Self {
        let mut set = Self {
            words: Vec::new(),
            tail: Some(interval),
        };
        set.reserve(positions);
        set
    }

    /// A set with NO stops, materialised far enough to answer for `positions` of them.
    ///
    /// The line-stop power-on state: ECMA-48 defines no default line tab stops, and neither VT100
    /// nor any terminal since has shipped one.
    #[must_use]
    pub fn none(positions: u16) -> Self {
        let mut set = Self {
            words: Vec::new(),
            tail: None,
        };
        set.reserve(positions);
        set
    }

    /// Materialise enough words to answer for `positions` positions, filling anything newly
    /// covered from the current tail rule.
    ///
    /// GROWS only. A shrinking resize keeps the bits it has, so narrowing a pane and widening it
    /// again restores the stops rather than the eight-column default — see the module docs.
    pub fn reserve(&mut self, positions: u16) {
        let needed = positions.div_ceil(BITS) as usize;
        while self.words.len() < needed {
            let base = self.words.len() as u16 * BITS;
            self.words.push(self.tail_word(base));
        }
    }

    /// The word covering the `BITS` positions from `base`, per the tail rule alone.
    ///
    /// Computed bit by bit rather than by a repeating mask: an interval that does not divide
    /// [`BITS`] has a pattern that shifts from word to word, and a mask written for eight would be
    /// silently wrong for every other interval a future sequence might ask for.
    fn tail_word(&self, base: u16) -> u64 {
        let Some(interval) = self.tail else {
            return 0;
        };
        let mut word = 0u64;
        for bit in 0..BITS {
            let Some(pos) = base.checked_add(bit) else {
                break;
            };
            if pos.is_multiple_of(interval.get()) {
                word |= 1u64 << bit;
            }
        }
        word
    }

    /// Whether `pos` is a stop.
    #[must_use]
    pub fn is_stop(&self, pos: u16) -> bool {
        let word = (pos / BITS) as usize;
        match self.words.get(word) {
            Some(bits) => bits & (1u64 << (pos % BITS)) != 0,
            // Past what is materialised, the tail rule is the answer. Reached only by a query
            // outside the geometry the emulator reserved for; the set stays total regardless.
            None => self.tail.is_some_and(|n| pos.is_multiple_of(n.get())),
        }
    }

    /// Put a stop at `pos` (HTS, or CTC 0 / CTC 1).
    pub fn set(&mut self, pos: u16) {
        self.reserve(pos.saturating_add(1));
        self.words[(pos / BITS) as usize] |= 1u64 << (pos % BITS);
    }

    /// Take the stop at `pos` away (TBC 0 / TBC 1, CTC 2 / CTC 3). A position that was not a stop
    /// is unchanged.
    pub fn unset(&mut self, pos: u16) {
        self.reserve(pos.saturating_add(1));
        self.words[(pos / BITS) as usize] &= !(1u64 << (pos % BITS));
    }

    /// Take EVERY stop away — the ones materialised and the ones not yet (TBC 3 / TBC 4 / TBC 5,
    /// CTC 4 / CTC 5 / CTC 6).
    ///
    /// The tail goes with them, so a later widen brings in columns with no stops. "All" said by an
    /// application that could only see eighty columns still means all: it is a statement about the
    /// device, not about the window it happened to be in.
    pub fn clear(&mut self) {
        self.words.fill(0);
        self.tail = None;
    }

    /// Put the set back to a stop every `interval` positions (DECST8C, and the power-on state a RIS
    /// restores), materialised and tail alike.
    pub fn reset_every(&mut self, interval: NonZeroU16) {
        self.tail = Some(interval);
        for word in 0..self.words.len() {
            let base = word as u16 * BITS;
            self.words[word] = self.tail_word(base);
        }
    }

    /// The next stop strictly after `from`, or `ceiling` when there is none up to it.
    ///
    /// Landing on `ceiling` rather than refusing is what a tab means with no stop ahead of it: HT
    /// walks to the right margin, and the caller has already decided which margin that is. A cursor
    /// already at or past `ceiling` does not move.
    #[must_use]
    pub fn next_after(&self, from: u16, ceiling: u16) -> u16 {
        let mut pos = from;
        while pos < ceiling {
            pos += 1;
            if self.is_stop(pos) {
                return pos;
            }
        }
        // The walk ran out: at `ceiling` when it started below it, still at `from` when it did not.
        pos
    }

    /// The last stop strictly before `from`, or `floor` when there is none down to it. The mirror
    /// of [`Self::next_after`]; a cursor already at or below `floor` does not move.
    #[must_use]
    pub fn prev_before(&self, from: u16, floor: u16) -> u16 {
        let mut pos = from;
        while pos > floor {
            pos -= 1;
            if self.is_stop(pos) {
                return pos;
            }
        }
        pos
    }
}

/// Both tabulation stop sets of a terminal, which reset and resize together.
///
/// They are one type because they are one piece of device state: a RIS restores both, DECSTR
/// touches neither (VT510 does not list tab stops among the settings a soft reset returns to
/// default), and a resize must grow both or a taller pane would answer CVT from a table that had
/// never heard of its new rows.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct TabStops {
    /// Character (column) tab stops — HT, CHT, CBT.
    pub columns: StopSet,
    /// Line (row) tab stops — CVT.
    pub lines: StopSet,
}

/// The power-on character tab stop interval: a stop every eight columns.
///
/// The VT100 default, and what DECST8C (`CSI ? 5 W`) means by "8 columns".
pub const DEFAULT_TAB_INTERVAL: NonZeroU16 = NonZeroU16::new(8).expect("8 is not zero");

impl TabStops {
    /// The power-on state for a `cols` x `rows` screen: character stops every eight columns, no
    /// line stops.
    #[must_use]
    pub fn new(cols: u16, rows: u16) -> Self {
        Self {
            columns: StopSet::every(DEFAULT_TAB_INTERVAL, cols),
            lines: StopSet::none(rows),
        }
    }

    /// Grow both sets to cover a `cols` x `rows` screen, KEEPING every stop already set.
    ///
    /// The whole divergence from Ghostty lives in this one line of behaviour — see the module docs
    /// for why a multiplexer cannot afford a resize that resets them.
    pub fn resize(&mut self, cols: u16, rows: u16) {
        self.columns.reserve(cols);
        self.lines.reserve(rows);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every position a set answers `true` for, up to `limit`.
    fn stops(set: &StopSet, limit: u16) -> Vec<u16> {
        (0..limit).filter(|p| set.is_stop(*p)).collect()
    }

    #[test]
    fn the_power_on_set_stops_every_eight_columns() {
        let set = StopSet::every(DEFAULT_TAB_INTERVAL, 20);
        assert_eq!(stops(&set, 20), vec![0, 8, 16]);
    }

    #[test]
    fn a_line_set_starts_empty() {
        let set = StopSet::none(20);
        assert_eq!(stops(&set, 20), Vec::<u16>::new());
    }

    #[test]
    fn a_set_stop_is_found_and_an_unset_one_is_not() {
        let mut set = StopSet::every(DEFAULT_TAB_INTERVAL, 20);
        set.set(3);
        set.unset(8);
        assert_eq!(stops(&set, 20), vec![0, 3, 16]);
    }

    #[test]
    fn next_after_walks_to_the_stop_and_stops_at_the_ceiling_without_one() {
        let set = StopSet::every(DEFAULT_TAB_INTERVAL, 40);
        assert_eq!(set.next_after(0, 39), 8);
        assert_eq!(set.next_after(8, 39), 16);
        // No stop between 32 and the ceiling: the ceiling is where a tab lands.
        assert_eq!(set.next_after(33, 39), 39);
        // Already at or past the ceiling: no move.
        assert_eq!(set.next_after(39, 39), 39);
        assert_eq!(set.next_after(45, 39), 45);
    }

    #[test]
    fn prev_before_mirrors_next_after() {
        let set = StopSet::every(DEFAULT_TAB_INTERVAL, 40);
        assert_eq!(set.prev_before(20, 0), 16);
        assert_eq!(set.prev_before(16, 0), 8);
        // Column 0 is a stop, so a walk back from 5 finds it.
        assert_eq!(set.prev_before(5, 0), 0);
        // Already at or below the floor: no move.
        assert_eq!(set.prev_before(0, 0), 0);
        assert_eq!(set.prev_before(3, 10), 3);
    }

    #[test]
    fn a_cleared_set_has_no_stop_anywhere_including_columns_it_has_never_seen() {
        let mut set = StopSet::every(DEFAULT_TAB_INTERVAL, 20);
        set.clear();
        assert_eq!(stops(&set, 20), Vec::<u16>::new());
        // The tail went with them: widening brings in no stops either. This is the half a
        // materialised-bits-only implementation gets wrong.
        set.reserve(400);
        assert_eq!(stops(&set, 400), Vec::<u16>::new());
    }

    #[test]
    fn reset_every_restores_the_tail_as_well_as_the_bits() {
        let mut set = StopSet::every(DEFAULT_TAB_INTERVAL, 20);
        set.clear();
        set.reset_every(DEFAULT_TAB_INTERVAL);
        assert_eq!(stops(&set, 20), vec![0, 8, 16]);
        set.reserve(200);
        assert!(set.is_stop(192), "the tail was restored too");
    }

    #[test]
    fn an_interval_that_does_not_divide_a_word_keeps_its_pattern_across_the_boundary() {
        // 6 does not divide 64, so the pattern shifts from word to word: a repeating mask written
        // for the first word would put the second word's stops in the wrong columns.
        let six = NonZeroU16::new(6).expect("6 is not zero");
        let set = StopSet::every(six, 140);
        let expected: Vec<u16> = (0..140).filter(|p| p % 6 == 0).collect();
        assert_eq!(stops(&set, 140), expected);
    }

    #[test]
    fn a_widen_after_a_narrow_finds_the_stops_where_they_were_left() {
        let mut stops_set = StopSet::every(DEFAULT_TAB_INTERVAL, 200);
        stops_set.set(150);
        // Narrowing does not take the table away...
        stops_set.reserve(20);
        // ...so widening back finds the stop rather than the eight-column default.
        stops_set.reserve(200);
        assert!(stops_set.is_stop(150), "a stop survived narrow-then-widen");
        assert!(!stops_set.is_stop(151));
    }

    /// A position PAST the materialised range still gets an answer, and it is the tail rule's.
    ///
    /// The emulator reserves for its own geometry before asking anything, so this arm is not on any
    /// path a sequence takes today — which is exactly why it is worth pinning. A set that answered
    /// `false` for everything past its last word would look correct until the day a caller queried
    /// ahead of a resize, and then be wrong silently.
    #[test]
    fn a_position_past_the_materialised_range_answers_from_the_tail() {
        let narrow = StopSet::every(DEFAULT_TAB_INTERVAL, 8);
        assert!(
            narrow.is_stop(4096),
            "an untouched far column keeps the rule"
        );
        assert!(!narrow.is_stop(4097));
        let mut cleared = StopSet::every(DEFAULT_TAB_INTERVAL, 8);
        cleared.clear();
        assert!(
            !cleared.is_stop(4096),
            "and a cleared tail answers for the far columns too"
        );
    }

    /// Setting or clearing a stop BEYOND what is materialised grows the table rather than panicking
    /// on the index.
    #[test]
    fn setting_a_stop_past_the_materialised_range_grows_the_table() {
        let mut set = StopSet::none(8);
        set.set(300);
        assert!(set.is_stop(300));
        set.unset(300);
        assert!(!set.is_stop(300));
        // The stop at the very last representable position is reachable too: `reserve` adds one
        // whole word at a time, so `u16::MAX` is the position where a `base + bit` walk would
        // overflow if it were not checked.
        set.set(u16::MAX);
        assert!(set.is_stop(u16::MAX));
    }

    /// The whole-width table materialises without overflowing the position walk.
    ///
    /// `tail_word` walks `base + bit` in `u16`, and the final word covers the positions ending at
    /// `u16::MAX`; an unchecked add there wraps to 0 and would set a stop at the START of the set.
    #[test]
    fn materialising_the_last_word_does_not_wrap_around_to_the_first() {
        let mut set = StopSet::none(8);
        set.reserve(u16::MAX);
        assert!(!set.is_stop(0), "no stop was smuggled in at position 0");
        let mut grid = StopSet::every(DEFAULT_TAB_INTERVAL, 8);
        grid.reserve(u16::MAX);
        // 65528 is the last multiple of 8 a u16 can hold; 65535 is not one.
        assert!(grid.is_stop(65528));
        assert!(!grid.is_stop(u16::MAX));
    }

    #[test]
    fn a_resize_grows_both_axes_and_keeps_every_stop() {
        let mut tabs = TabStops::new(20, 5);
        tabs.columns.set(3);
        tabs.lines.set(2);
        tabs.resize(200, 60);
        assert!(
            tabs.columns.is_stop(3),
            "a character stop survived a resize"
        );
        assert!(tabs.lines.is_stop(2), "a line stop survived a resize");
        assert!(
            tabs.columns.is_stop(192),
            "the new columns carry the default interval"
        );
        assert!(
            !tabs.lines.is_stop(40),
            "the new rows carry the empty line-stop tail"
        );
    }
}
