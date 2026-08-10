//! What a blocked pane is ASKING, read off its screen as data.
//!
//! [`AgentState::Blocked`](crate::AgentState::Blocked) says a pane cannot continue until somebody
//! answers it. It does not say what the question was, and a supervisor — a person's policy, or a
//! plugin standing in for one — cannot answer a question it can only see as a bitmap. This module
//! turns the measured dialog shape into [`Question`]: the sentence the agent asked, and the
//! numbered options it will accept, each with the number a caller types to pick it.
//!
//! ## One authority, not two
//!
//! [`question`] is also the TEST the blocked rule fires on
//! ([`Test::ChoiceList`](crate::Test::ChoiceList)). That is deliberate and it is the whole reason
//! this replaced a regex rather than sitting beside one: a rule that says "blocked" and a parser
//! that says "no options here" would be two readers building the same thing out of the same rows,
//! which is the defect shape R344 spent a round on. With one function there is no state in which a
//! pane is reported blocked by a choice list nobody can enumerate.
//!
//! ## What the shape is, and where it was measured
//!
//! Six real dialogs across two independent agents (`claude`, in TypeScript and Ink; `codex`, in
//! Rust and `ratatui`) share exactly one shape, and every clause below is one of them rather than a
//! generalisation:
//!
//! * A **selection marker** on one option — `❯` (U+276F), `›` (U+203A) or `>` (U+003E). Three
//!   markers, two of them inside ONE agent, so the marker is not even a per-agent constant.
//! * `<n>.` at the start of an option's line, and the numbers run CONSECUTIVELY. The consecutive
//!   half is what a lone regex could not say, and it is what keeps two unrelated numbered lines
//!   from reading as a menu.
//! * **At least two** options. One numbered line is a prompt echo — a person typing `❯ 1. rewrite
//!   the parser` — and a menu with one choice is not a menu.
//! * A row indented PAST its option's number belongs to that option. Measured on both agents'
//!   model pickers, whose descriptions run onto a second row, and it is what separates a
//!   continuation from the footer below the list: every measured footer sits at or LEFT of the
//!   option indent (`Press enter to continue`, `Enter to confirm · Esc to cancel`), and every
//!   measured continuation sits right of it.
//!
//! ## A line, not a row
//!
//! The window is the same one the other rules read — the last N non-empty ROWS, because how far up
//! the screen a dialog sits is measured in rows (R344 settled that, and
//! `a_dialog_still_reads_as_blocked_when_every_line_of_it_wraps` pins it). What this reads out of
//! that window is LINES: a narrow pane tears `3. No, and tell Claude what to do differently` across
//! two rows, and an option whose label is half a word is not an option a policy can classify. The
//! join goes through [`Screen::row_share_text`], so it is the emulator's own arithmetic and not a
//! fourth guess at where a row's share of its line ends.

use sprag_vt::Screen;

/// The selection markers measured across the two built-in agents.
///
/// A CLASS and not a literal: `claude` marks with `❯`, while `codex` marks its sign-in picker with
/// `>` and its directory-trust dialog with `›` — two glyphs inside one agent, so a parser keyed on
/// the one glyph a probe happened to show is a parser about that probe.
const MARKERS: [char; 3] = ['❯', '›', '>'];

/// One option an agent is offering.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Choice {
    /// The number a caller types to pick it. Taken from the screen rather than from the option's
    /// position, because a list that has scrolled does not start at one.
    pub number: u32,
    /// What the option says: its own row plus every row indented under it, with runs of whitespace
    /// collapsed to one space — an agent aligns a description into a column, and the padding is
    /// layout rather than text.
    pub label: String,
    /// Whether the agent's selection marker is on this option — where a bare Enter would land, and
    /// so the answer a caller gets by doing nothing.
    pub selected: bool,
}

/// What a pane is blocked on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Question {
    /// The lines above the list, in reading order, trimmed — the sentence a policy classifies.
    ///
    /// Everything in the window above the first option, including the agent's own box rules, kept
    /// rather than filtered: which of those lines carries the meaning is the consumer's judgement,
    /// and a filter written here would be this crate deciding what a policy may read.
    pub asked: Vec<String>,
    /// The options, in screen order. Never fewer than two, and their numbers are consecutive.
    pub choices: Vec<Choice>,
}

impl Question {
    /// The option the marker is on — where Enter would land. Always `Some` for a [`Question`] that
    /// exists at all, because a marker is one of the conditions [`question`] requires; it is an
    /// `Option` because the field it reads is per-choice and nothing should have to re-assert the
    /// invariant to read it.
    #[must_use]
    pub fn selected(&self) -> Option<&Choice> {
        self.choices.iter().find(|choice| choice.selected)
    }

    /// The option numbered `number`, or `None` when the list does not offer it — the check a
    /// supervisor makes before injecting a digit, so a policy that answers "2" to a two-option
    /// dialog cannot silently answer nothing.
    #[must_use]
    pub fn choice(&self, number: u32) -> Option<&Choice> {
        self.choices.iter().find(|choice| choice.number == number)
    }
}

/// Read the choice list out of the last `window` non-empty rows of `screen`, or `None` when there
/// is not one there.
///
/// `None` is the answer for every pane that is not showing a menu, which is nearly all of them, and
/// it is reached without allocating a line for a screen whose window holds no numbered option at
/// all.
#[must_use]
pub fn question(screen: &Screen, window: u16) -> Option<Question> {
    let lines = bottom_logical(screen, window);
    let (start, choices) = choice_run(&lines)?;
    Some(Question {
        asked: lines[..start]
            .iter()
            .map(|line| line.trim().to_owned())
            .filter(|line| !line.is_empty())
            .collect(),
        choices,
    })
}

/// The last `window` non-empty rows of the visible screen, in reading order, with the rows a line
/// soft-wrapped across joined back into one entry.
///
/// The WINDOW is rows and the CONTENT is lines — see the module docs for why those are different
/// questions. A line whose head sits above the window arrives as its tail, which is honest: the
/// window is what the caller asked to see.
fn bottom_logical(screen: &Screen, window: u16) -> Vec<String> {
    let mut rows: Vec<u16> = Vec::with_capacity(window as usize);
    for row in (0..screen.rows()).rev() {
        if rows.len() == window as usize {
            break;
        }
        if !screen.row_text(row).trim().is_empty() {
            rows.push(row);
        }
    }
    rows.reverse();

    let mut lines: Vec<String> = Vec::new();
    let mut open: Option<String> = None;
    for (index, &row) in rows.iter().enumerate() {
        let text = open.take().unwrap_or_default() + &screen.row_share_text(row);
        // The line runs on only if the very next ROW carries it AND that row is in the window; a
        // continuation the window cut off closes the line here, exactly as the end of the screen
        // does.
        if screen.wrapped(row) && rows.get(index + 1) == Some(&(row + 1)) {
            open = Some(text);
        } else {
            lines.push(text.trim_end().to_owned());
        }
    }
    lines.extend(open.map(|text| text.trim_end().to_owned()));
    lines
}

/// One option's opening line, taken apart.
struct Header {
    number: u32,
    marked: bool,
    /// The COLUMN the number starts at — what a continuation of this option must be indented past.
    column: usize,
    /// The text after the `N.`, front-trimmed.
    rest: String,
}

/// Read `line` as an option's opening line, or `None` when it is not one.
fn header(line: &str) -> Option<Header> {
    let mut column = indent(line);
    let mut rest = line.trim_start();

    let marked = rest.chars().next().is_some_and(|ch| MARKERS.contains(&ch));
    if marked {
        // A marker binds to the option it precedes, so it must be followed by space. Without that
        // clause `>1` in ordinary output would open a menu.
        let after = &rest[rest.chars().next()?.len_utf8()..];
        let gap = after.chars().count() - after.trim_start().chars().count();
        if gap == 0 {
            return None;
        }
        column += 1 + gap;
        rest = after.trim_start();
    }

    let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
    if digits.is_empty() {
        return None;
    }
    let after_digits = &rest[digits.len()..];
    let body = after_digits.strip_prefix('.')?;
    // `1.foo` is not an option — every measured agent puts a space after the dot, and without the
    // clause a version number opens a menu.
    if !body.is_empty() && !body.starts_with(char::is_whitespace) {
        return None;
    }
    Some(Header {
        number: digits.parse().ok()?,
        marked,
        column,
        rest: body.trim_start().to_owned(),
    })
}

/// How many columns of leading whitespace `line` has.
fn indent(line: &str) -> usize {
    line.chars().count() - line.trim_start().chars().count()
}

/// The choice list in `lines`, as `(the line index it starts at, its options)`.
///
/// The LAST qualifying run wins. A dialog is bottom-anchored, so when a window holds a stale menu
/// above a live one the live one is the lower — and the alternative, taking the first, would answer
/// with a list whose numbers no longer do anything.
fn choice_run(lines: &[String]) -> Option<(usize, Vec<Choice>)> {
    let mut best: Option<(usize, Vec<Choice>)> = None;
    let mut run: Vec<Choice> = Vec::new();
    let mut start = 0usize;
    let mut column = 0usize;

    /// End the run being built: keep it if it is a menu, and empty it either way.
    fn close(run: &mut Vec<Choice>, start: usize, best: &mut Option<(usize, Vec<Choice>)>) {
        if qualifies(run) {
            *best = Some((start, std::mem::take(run)));
        }
        run.clear();
    }

    for (index, line) in lines.iter().enumerate() {
        if let Some(head) = header(line) {
            let consecutive = run
                .last()
                .is_some_and(|last| head.number == last.number + 1);
            if !consecutive {
                close(&mut run, start, &mut best);
                start = index;
            }
            column = head.column;
            run.push(Choice {
                number: head.number,
                label: collapse(&head.rest),
                selected: head.marked,
            });
        } else if let Some(last) = run.last_mut()
            && indent(line) > column
        {
            // A row indented past its option's number is that option's own second row.
            let extra = collapse(line);
            if !extra.is_empty() {
                if !last.label.is_empty() {
                    last.label.push(' ');
                }
                last.label.push_str(&extra);
            }
        } else {
            close(&mut run, start, &mut best);
        }
    }
    close(&mut run, start, &mut best);
    best
}

/// Whether a run of numbered lines is a menu: at least two options, and one of them marked.
///
/// The numbers being consecutive is enforced where the run is built, because a break in the
/// sequence starts a NEW run rather than spoiling the one before it.
fn qualifies(run: &[Choice]) -> bool {
    run.len() >= 2 && run.iter().any(|choice| choice.selected)
}

/// `text` with each run of whitespace collapsed to one space, and the ends trimmed.
fn collapse(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use sprag_vt::{Emulator, VtPort};

    /// The MECHANISM's tests. What the mechanism is a mechanism FOR — six captured dialogs from two
    /// real agents — is asserted beside those fixtures, in the crate root's tests.
    fn asked(lines: &[&str]) -> Option<Question> {
        let mut em = Emulator::new(80, 24);
        em.advance(lines.join("\r\n").as_bytes());
        question(em.screen(), 12)
    }

    #[test]
    fn a_marked_pair_of_consecutive_options_is_a_menu() {
        let q = asked(&["Pick one:", "❯ 1. first", "  2. second"]).expect("a menu");
        assert_eq!(q.asked, vec!["Pick one:".to_owned()]);
        assert_eq!(q.choices.len(), 2);
        assert_eq!(q.selected().map(|c| c.number), Some(1));
        assert_eq!(q.choice(2).map(|c| c.label.as_str()), Some("second"));
        assert_eq!(q.choice(3), None, "a number nobody offered is not on offer");
    }

    /// The clause the retired regex did not have. Two numbered lines that are not a sequence are
    /// two numbered lines.
    #[test]
    fn numbers_that_do_not_run_consecutively_are_not_one_menu() {
        assert!(asked(&["❯ 1. first", "  3. third"]).is_none());
    }

    /// A break in the sequence starts a NEW run rather than spoiling the one before it, so a live
    /// menu below a stale one is still read.
    #[test]
    fn the_lower_of_two_menus_wins_because_a_dialog_is_bottom_anchored() {
        let q = asked(&[
            "❯ 1. stale one",
            "  2. stale two",
            "and then something else happened",
            "❯ 1. live one",
            "  2. live two",
        ])
        .expect("a menu");
        assert_eq!(q.choice(1).map(|c| c.label.as_str()), Some("live one"));
        assert_eq!(
            q.asked.last().map(String::as_str),
            Some("and then something else happened"),
            "everything above the LIVE run is what was asked",
        );
    }

    /// A marker binds to the option it precedes, and a bare number is not an option.
    #[test]
    fn a_marker_with_no_gap_and_a_number_with_no_dot_open_nothing() {
        assert!(asked(&[">1. first", " 2. second"]).is_none(), "no gap");
        assert!(asked(&["❯ 1 first", "  2 second"]).is_none(), "no dot");
        assert!(
            asked(&["❯ 1.first", "  2.second"]).is_none(),
            "no space after the dot: a version number is not an option",
        );
    }

    /// An unmarked list is a list nobody is standing on — a numbered paragraph, not a dialog.
    #[test]
    fn a_numbered_list_with_no_marker_is_not_a_menu() {
        assert!(asked(&["1. buy milk", "2. buy bread"]).is_none());
    }

    /// The indent rule, in both directions, on one screen: the row indented PAST the number joins
    /// its option, and the row at or left of it ends the list.
    #[test]
    fn indent_separates_a_continuation_from_the_footer_below_the_list() {
        let q = asked(&[
            "  ❯ 1. first",
            "       and its second row",
            "    2. second",
            "  Press enter to continue",
        ])
        .expect("a menu");
        assert_eq!(
            q.choice(1).map(|c| c.label.as_str()),
            Some("first and its second row"),
        );
        assert_eq!(
            q.choice(2).map(|c| c.label.as_str()),
            Some("second"),
            "the footer sits at the option indent, so it belongs to nobody",
        );
    }

    /// The window is the last N non-empty ROWS, unchanged from every other rule — so a caller
    /// asking about a smaller window than the dialog occupies gets no dialog rather than half of
    /// one.
    #[test]
    fn a_window_that_does_not_reach_the_marker_reads_no_menu() {
        let mut em = Emulator::new(80, 24);
        em.advance(b"\xe2\x9d\xaf 1. first\r\n  2. second\r\n  3. third");
        assert!(question(em.screen(), 12).is_some(), "the whole list");
        assert!(
            question(em.screen(), 2).is_none(),
            "a window holding options 2 and 3 holds no marker",
        );
    }
}
