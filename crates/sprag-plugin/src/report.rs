//! **WHAT A PANE HAS PRODUCED SINCE A MARK, AND WHAT AN AGENT'S REPORT IS INSIDE IT.**
//!
//! Two plugins here drive a peer and publish what it said back —
//! [`Agent`](crate::agent::Agent), whose caller reads the reply AS THE MODEL'S ANSWER, and
//! [`AiLoop`](crate::ai_loop::AiLoop), whose closing report is the one account of a run that
//! outlives the sessions it was written in. They shared no code and one of them had no reader at
//! all.
//!
//! # ⚠⚠⚠ The reader was chosen by measurement, against a live agent, and the habit lost
//!
//! Every *what happened since* reader in the outer driver — `said_done`, `judged`, `proposed` —
//! compared the RENDERING through [`RowTrail`], so a fourth one would have been written that way
//! without a thought. `what_a_live_agents_report_looks_like_to_a_reader` asked a real `claude` for
//! sixty labelled lines on a forty-row pane and read both:
//!
//! | reader | what it came back with |
//! |---|---|
//! | the rendering ([`RowTrail`]) | **37 rows, opening at `LINE-29`** — the first twenty-eight lines had scrolled and were simply not there |
//! | the address ([`PaneOutputLines`]) | **66 logical lines, `lost: 0`**, opening at `LINE-1` |
//!
//! **A truncated report is worse than a missing one, because nothing in it says it is truncated** —
//! [`Agent::capture`](crate::agent::Agent) argued exactly that about the same two readers and called
//! the trail *"the degradation"*. That sentence was written about a one-shot CLI printing to a
//! cooked-mode pty; this measurement is the first time it was asked of **a full-screen TUI that
//! repaints in place**, which is what a loop's peer is, and the answer held.
//!
//! So the address is what a report is read through, and the trail stays as the fallback for a host
//! that cannot number its lines — named as a degradation rather than as an equivalent.
//!
//! ⚠⚠⚠ **AND `said_done` FOLLOWED IT, ONE ROUND LATER AND FOR A SHARPER REASON** (register item
//! 270). Asking *"is the marker the whole ROW?"* is asking a question about a width nobody chose:
//! `done_instruction` is 109 characters with its marker at 92, so a pane 23, 46 or 92 columns wide
//! breaks that sentence exactly at the marker. It now reads [`Since`] — the same mark, the same
//! address — so **a run's convergence and its account are read out of one text**, which is what
//! stops them coming to disagree about what the agent said.
//!
//! # ⚠⚠ What else is in those lines, measured off the same probe
//!
//! The address is not a transcript of the model. Verbatim, in order, from that live run:
//!
//! ```text
//! [  0] "  not number them any other way and do not add commentary."   <- the ECHO, re-wrapped
//! [  1] ""
//! [  2] "● LINE-1"                                                     <- the reply, decorated
//! ...
//! [ 61] "  LINE-60"
//! [ 62] ""
//! [ 63] "✻ Cogitated for 3s"                                           <- the TUI's own chrome
//! [ 64] ""
//! [ 65] "────────────────────────────────────────────────────────..."  <- the composer's border
//! ```
//!
//! Three facts a capture has to be built from, and each decides a rule below: the prompt this run
//! typed comes back **re-wrapped**, so an exact-prefix echo rule cannot find it; the reply carries
//! the agent's own decoration; and the furniture is at the EDGES.
//!
//! [`PaneOutputLines`]: crate::access::PaneOutputLines

use sprag_terminal::PaneId;

use crate::access::{PaneAccess, RowTrail};

/// Where a turn's output begins — an ADDRESS when the host can number its lines, and a mark on the
/// rendering when it cannot.
///
/// ⚠ The two are not equivalent and this type does not pretend they are: see [`Produced::lost`],
/// which is a real count through one and an admitted UNKNOWN through the other.
pub(crate) enum Since {
    /// The absolute line number the output begins at.
    Line(u64),
    /// What the rows held before it — the degradation.
    Rows(RowTrail),
}

/// **NOTHING MARKED YET** — an empty trail, whose meaning is *everything the pane holds is new*.
///
/// The same value and the same reason as the `judged` trail beside it in
/// [`Session`](crate::outer): a session is constructed before a byte has been typed into its pane,
/// and every prompt marks afresh immediately before injecting. So this is what a mark is between a
/// pane existing and it being spoken to, and no reader can reach it — a report is read at the end
/// of a turn, and a turn begins with a prompt.
impl Default for Since {
    fn default() -> Self {
        Self::Rows(RowTrail::default())
    }
}

impl Since {
    /// Mark `pane` right now.
    ///
    /// ⚠ `u64::MAX` MARKS WITHOUT TAKING: it is past every line, so nothing is yielded and `next`
    /// is the address the output will start at.
    pub(crate) fn mark(panes: &dyn PaneAccess, pane: PaneId) -> Self {
        panes
            .output_lines()
            .and_then(|stream| stream.pane_lines_since(pane, u64::MAX))
            .map_or_else(
                || Self::Rows(RowTrail::mark(panes, pane)),
                |since| Self::Line(since.next),
            )
    }

    /// What `pane` has produced since this mark.
    pub(crate) fn taken(&self, panes: &dyn PaneAccess, pane: PaneId) -> Produced {
        match self {
            Self::Line(cursor) => panes
                .output_lines()
                .and_then(|stream| stream.pane_lines_since(pane, *cursor))
                .map_or_else(
                    // ⚠⚠⚠⚠⚠ **THE FALLBACK IS NOT AN EMPTY READ, IT IS NO READ** — register item
                    // 431. This arm is reached when the host serves no line stream at all or the
                    // stream would not answer for this pane, and `Produced::default()` alone made
                    // it byte-identical to a turn that printed nothing. Every consumer then read
                    // *the agent was silent* about a pane nobody managed to look at.
                    || Produced {
                        unread: true,
                        ..Produced::default()
                    },
                    |since| Produced {
                        lines: since.lines,
                        partial: since.partial,
                        lost: since.lost,
                        restarted: since.restarted,
                        // ⚠ The read HAPPENED, whatever it found — a `since` in hand is the proof,
                        // and an empty one is a genuine measurement of a quiet turn.
                        unread: false,
                    },
                ),
            Self::Rows(trail) => Produced {
                lines: trail.fresh(panes, pane),
                // ⚠ A rendering comparison cannot report a loss it cannot see — a scrolled-away row
                // is simply not there to be counted — and it holds no unfinished line either,
                // because a row is whatever is on the grid. Both are the degradation this fallback
                // is named as, not a claim about the pane.
                partial: String::new(),
                lost: 0,
                // ⚠ And a rendering comparison has no address space to lose, so there is nothing
                // here that COULD restart — `false` is the truth rather than a default.
                restarted: false,
                // ⚠ Nor a world in which the look did not happen: the grid is always there to be
                // compared, so this is the truth here too. See the field's own doc.
                unread: false,
            },
        }
    }
}

/// What a pane produced since a [`Since`], and what could not be in it.
#[derive(Default)]
pub(crate) struct Produced {
    /// The complete logical lines, oldest first.
    pub(crate) lines: Vec<String>,
    /// The line the pane is STILL WRITING — see [`sprag_vt::LinesSince::partial`]. A caller must
    /// earn the right to use it by establishing that the child has ended, and a long-lived agent
    /// CLI never does: its unfinished line is a prompt box, which is furniture.
    pub(crate) partial: String,
    /// Complete lines the retained history evicted before this read — `0` in the ordinary case, and
    /// `0` also means UNKNOWN through [`Since::Rows`].
    pub(crate) lost: u64,
    /// **THE READ DID NOT HAPPEN AT ALL** — register item 431, and the difference between *the pane
    /// produced nothing* and *nothing here could look at the pane*.
    ///
    /// # ⛔⛔⛔⛔⛔ An empty `lines` is two facts and this is the one that says which
    ///
    /// [`Since::taken`]'s address arm falls back to [`Default`] when the pane's line stream cannot
    /// be reached — no `output_lines` surface, or a `pane_lines_since` that answered nothing — and
    /// that fallback is byte-identical to a turn in which the agent printed nothing. **Measured
    /// 2026-08-28**: a live run judged ~400 turns on `0 line(s)` over five hours while its agent
    /// was demonstrably working and committing nothing, and a second run burned its whole budget
    /// (`exhausted (cost) after 183 iterations`) with six of its last eleven judgements reading 0.
    ///
    /// ⚠⚠ It is the workspace's own standing rule in a fourth place: **`None` (not counted) is not
    /// `Some(0)`**. A zero that could not be read and a zero that was read are the same word to
    /// every consumer, and this run's document routes on the difference — a judgement told *the
    /// agent printed nothing* prompts again as if the turn were ordinary.
    ///
    /// ⚠ `false` through [`Since::Rows`]: a rendering comparison always has a grid to compare, so
    /// there is no world there in which the look did not happen. That is the truth rather than a
    /// default, exactly as [`restarted`](Self::restarted) beside it.
    pub(crate) unread: bool,
    /// **THE ADDRESSES CHANGED UNDER THIS READER**, so `lines` starts from what the pane retains
    /// rather than from where this mark left off — [`sprag_vt::LinesSince::restarted`].
    ///
    /// ⚠⚠⚠ What it costs a consumer, and why it cannot be folded into [`lost`](Self::lost): after a
    /// restart the lines handed back are *what this screen holds*, not *what arrived since the
    /// mark*. Any reader whose rule is **"in THIS turn"** has lost that guarantee for this read —
    /// the text may predate the turn entirely. `lost` says lines are missing; this says the ones
    /// present may not be the ones asked for.
    ///
    /// ⚠ Always `false` through [`Since::Rows`], which compares renderings and has no address space
    /// to lose — the same shape as `lost` being an admitted unknown there.
    pub(crate) restarted: bool,
}

/// **WHAT THE AGENT SAID, OUT OF WHAT THE PANE PRINTED** — `lines` with this run's own echo and the
/// terminal's furniture taken off the edges, or [`None`] when nothing is left that a person would
/// read as an answer.
///
/// # ⚠⚠⚠ Two rules, and both fail in the same direction on purpose
///
/// **Deleting a line of an answer is worse than leaving a line that was not one**, because only the
/// first is unrecoverable — [`without_own_echo`](crate::agent) argued it and this obeys it. So each
/// rule drops only what is PROVABLY not the agent's:
///
/// * **THE ECHO IS A LEADING RUN OF WHAT `asked` CONTAINS.** An agent CLI paints the prompt into
///   its composer and re-wraps it to the pane's width, so the row that reaches the line store is a
///   FRAGMENT of what was typed — measured as `"  not number them any other way and do not add
///   commentary."` from a prompt three lines long, sitting at index 0 with the reply behind it.
///   [`without_own_echo`](crate::agent)'s leading-EXACT rule cannot see a fragment, so the
///   comparison is `asked.contains(line)`; its LEADING half is kept, and keeping it is not
///   conservatism.
///   ⚠⚠⚠ **DROPPING IT DELETED AN ANSWER, MEASURED.** The first draft discounted a matching line
///   wherever it appeared, on the invented ground that a re-wrap could put a fragment anywhere. The
///   live fixture said otherwise within the hour: an agent asked for lines up to `LINE-60` answered
///   `LINE-60`, the prompt naming the last line CONTAINED that string, and **the agent's own final
///   line was deleted from its report**. The composer paints before the peer replies; once the peer
///   has spoken, what follows is the peer's.
/// * **FURNITURE GOES FROM THE EDGES ONLY.** A line with no word character — a box rule, a blank —
///   carries nothing a reader wants, and the measurement found them wrapping the reply on both
///   sides. Trimmed at the ends and KEPT in the middle, because an interior blank line is a
///   paragraph break in somebody's report and dropping those would rewrite it.
///
/// # ⚠⚠ What is deliberately NOT stripped, and why each would be a guess
///
/// * **The agent's decoration** (`● ` on the first line of a reply, two spaces on the rest). It is
///   uniform today and stripping it would mean deciding that a leading indent is never the report's
///   own — false the moment a report contains a code block or a nested list.
/// * **The TUI's own status lines** (`✻ Cogitated for 3s`). They carry words, so no rule here can
///   tell them from a sentence; telling them apart needs an agent's manifest, which is
///   `sprag-detect`'s knowledge and not this module's. Leaving them costs a reader one line of
///   noise, and a vocabulary of chrome kept here would rot the first time an agent reworded itself.
pub(crate) fn account(produced: &Produced, asked: &str) -> Option<String> {
    let said = said(&produced.lines, asked)?;
    if produced.lost == 0 {
        return Some(said);
    }
    Some(format!("{}\n{said}", lost_line(produced.lost)))
}

/// **THE ONE PLACE THIS MODULE PUTS WORDS OF ITS OWN INTO SOMEBODY ELSE'S ACCOUNT**, and it is
/// there because the alternative is a lie.
///
/// A pane's retained history is finite ([`sprag_vt::DEFAULT_SCROLLBACK_LINES`]), so a report longer
/// than it can hold arrives with its opening evicted — and **a truncated account is worse than a
/// missing one, because nothing in it says it is truncated**. The step's note carries the count as
/// well, which is where [`Agent`](crate::agent::Agent) puts it; a consumer reading the captured
/// content ALONE would otherwise be handed a middle and told it was a whole account.
///
/// ⚠ It opens with `sprag:` so a reader can tell at a glance which words are the agent's. Nothing
/// parses it, deliberately: a caller who wants the number as a number wants the run's journal.
fn lost_line(lost: u64) -> String {
    format!(
        "[sprag: this pane's retained history evicted {lost} line(s) before the report was read; \
         what follows begins mid-account]"
    )
}

fn said(lines: &[String], asked: &str) -> Option<String> {
    let words = |line: &str| line.chars().any(char::is_alphanumeric);
    // Everything before the agent's first word of its own: blanks, box rules, and this run's own
    // prompt coming back. The three are one predicate because they are one region — what the
    // terminal put on the screen between the question going in and the answer starting.
    let before = |line: &&String| !words(line) || asked.contains(line.trim());
    let first = lines.iter().position(|line| !before(&line))?;
    let last = lines.iter().rposition(|line| words(line))?;
    Some(
        lines[first..=last]
            .iter()
            .map(|line| line.trim_end())
            .collect::<Vec<_>>()
            .join("\n"),
    )
}

#[cfg(test)]
mod tests {
    use super::{Produced, account, said};

    /// The live probe's own lines, verbatim — abridged in the middle only, and every EDGE kept.
    ///
    /// ⚠⚠⚠ **THE FIXTURE'S INPUT IS THE PRODUCT'S INPUT.** These are the strings
    /// `pane_lines_since` handed back from a real `claude` pane at 120 columns, copied out of
    /// `what_a_live_agents_report_looks_like_to_a_reader`'s printed walk. A hand-written
    /// approximation of *"what a TUI prints"* would be this test agreeing with whatever the rules
    /// below happen to do — R383's rule, which cost that round a capture that was not the parser's
    /// input at all.
    const LIVE: &[&str] = &[
        "  not number them any other way and do not add commentary.",
        "",
        "● LINE-1",
        "  LINE-2",
        "  LINE-59",
        "  LINE-60",
        "",
        "✻ Cogitated for 3s",
        "",
        "────────────────────────────────────────────────────────────────────────────",
    ];

    /// The prompt that produced [`LIVE`], as this run typed it.
    const ASKED: &str = "Print exactly 60 lines and nothing else. Line n must be LINE-n, so the \
                         first is LINE-1 and the last is LINE-60. Do not number them any other way \
                         and do not add commentary.";

    fn lines(of: &[&str]) -> Vec<String> {
        of.iter().map(|line| (*line).to_owned()).collect()
    }

    /// ⚠⚠⚠ **THE REPORT OPENS ON THE AGENT'S FIRST WORD AND CLOSES ON ITS LAST** — asserted on the
    /// exact lines a live agent produced.
    ///
    /// Three things are being held at once, and each was a separate way of publishing something
    /// false to whoever reads a run's account:
    ///
    /// * the ECHO is gone, so the caller's own instruction is not returned as the agent's report;
    /// * the FURNITURE is gone from both ends, so the account does not begin or end in a box rule;
    /// * everything BETWEEN them survives, decoration and all.
    #[test]
    fn a_report_is_what_the_agent_said_between_this_runs_echo_and_the_terminals_furniture() {
        let said = said(&lines(LIVE), ASKED).expect("a live agent's reply is a report");
        assert_eq!(
            said, "● LINE-1\n  LINE-2\n  LINE-59\n  LINE-60\n\n✻ Cogitated for 3s",
            "⚠⚠⚠ the report must open on the agent's first word and close on its last, keeping \
             what is between: {said:?}",
        );
        assert!(
            !said.contains("do not add commentary"),
            "⚠⚠⚠ THE ECHO IS THE CALLER'S OWN SENTENCE COMING BACK AS THE AGENT'S REPORT. A live \
             agent re-wraps the prompt into its composer, so the line store holds a FRAGMENT of it \
             — matching it needs `asked.contains(line)` and not the other way round: {said:?}",
        );
    }

    /// ⚠⚠⚠ **A LINE OF THE ANSWER THAT HAPPENS TO APPEAR IN THE QUESTION IS NOT AN ECHO** — and
    /// this gate exists because the first draft of the rule above DELETED ONE, measured on the
    /// fixture beside it within an hour of the rule being written.
    ///
    /// That draft discounted a matching line **wherever it appeared**, on the invented ground that
    /// a re-wrapping composer could put its fragment anywhere in the region. It was never measured:
    /// the live probe found the echo at index 0 with the whole reply behind it. What the invention
    /// cost was immediate, and it is the exact failure this module is written to refuse — the
    /// agent, asked for lines up to `LINE-60`, answered `LINE-60`; the prompt named it; **the
    /// agent's own last line vanished from its account.**
    ///
    /// ⚠ So the echo is the LEADING run, and the discriminator is not *does the question mention
    /// this?* but *had the peer started speaking yet?*
    #[test]
    fn a_line_the_agent_wrote_survives_being_a_phrase_the_question_also_used() {
        let said = said(
            &lines(&["  and the last is LINE-60.", "● LINE-59", "  LINE-60"]),
            ASKED,
        )
        .expect("a report");
        assert_eq!(
            said, "● LINE-59\n  LINE-60",
            "⚠⚠⚠ THE LEADING FRAGMENT IS THE ECHO AND THE ANSWER BEHIND IT IS THE AGENT'S, even \
             where the two share a phrase. A rule that discounted the phrase everywhere deleted \
             `LINE-60` — an unrecoverable loss — to avoid keeping a line that was never a risk: \
             {said:?}",
        );
    }

    /// ⚠⚠⚠ **AN ACCOUNT THAT LOST ITS OPENING SAYS SO IN ITSELF** — the one place this module puts
    /// words of its own into somebody else's report, and the reason is that the alternative is a
    /// lie a reader cannot detect.
    ///
    /// A pane's retained history is a thousand lines. A report longer than that arrives with its
    /// beginning evicted, and everything about the remainder looks exactly like a complete account:
    /// it starts on a sentence, it ends on a sentence, and **nothing in it says it is a middle**.
    /// [`Agent::capture`](crate::agent::Agent) made this argument about the same field and then put
    /// the count only in its step note — one reader of a hazard is not a reader of it — so the run's
    /// journal carries it AND the content does.
    ///
    /// ⚠ It is a unit rather than a run because a loop whose closing report exceeds a thousand
    /// lines is not a fixture anybody should have to build to keep this honest.
    #[test]
    fn an_account_whose_opening_the_pane_evicted_carries_that_fact_in_its_own_text() {
        let produced = |lost| Produced {
            lines: lines(&["what changed", "what is left"]),
            partial: String::new(),
            lost,
            // ⚠ This fixture varies `lost` alone. A restart is the other discontinuity (item 366)
            // and staging it here would blur which one the account's sentence is about.
            restarted: false,
            // ⚠ The read HAPPENED here — `lines` above is the proof — so this fixture is staging an
            // evicted opening rather than register item 431's absent reader.
            unread: false,
        };
        let whole = produced(0);
        let cut = produced(7);
        assert_eq!(
            account(&whole, "irrelevant").as_deref(),
            Some("what changed\nwhat is left"),
            "⚠ the control: an account the pane held whole is the agent's words and nothing else",
        );
        let cut = account(&cut, "irrelevant").expect("an account");
        assert!(
            cut.starts_with("[sprag:") && cut.contains('7') && cut.ends_with("what is left"),
            "⚠⚠⚠ A TRUNCATED ACCOUNT MUST SAY SO IN THE TEXT A READER IS HANDED. Without it, a \
             report that begins mid-sentence is indistinguishable from a complete one: {cut:?}",
        );
    }

    /// ⚠⚠ **AN INTERIOR BLANK IS A PARAGRAPH BREAK AND SURVIVES**, where the same blank at an edge
    /// is furniture. The rule is about POSITION, and a capture that trimmed blanks everywhere would
    /// silently re-flow somebody's report into one paragraph.
    #[test]
    fn a_blank_line_inside_a_report_is_part_of_it_and_the_same_blank_at_its_edge_is_not() {
        let said = said(
            &lines(&["", "what changed", "", "what is left", ""]),
            "irrelevant",
        )
        .expect("a report");
        assert_eq!(
            said, "what changed\n\nwhat is left",
            "⚠⚠ the paragraph break is the agent's and the edges are the terminal's: {said:?}",
        );
    }

    /// ⚠⚠⚠ **A TURN THAT SAID NOTHING PUBLISHES NOTHING** — the direction this whole module has to
    /// fail in. A capture that answered `Some("")` would tell a reader the agent produced an EMPTY
    /// report, which is a claim; `None` says there is no report, which is the truth about a pane
    /// that only ever painted its own furniture back.
    #[test]
    fn a_region_holding_nothing_but_this_runs_echo_and_furniture_is_no_report_at_all() {
        assert_eq!(
            said(
                &lines(&["", "  summarise what changed", "────", ""]),
                "summarise what changed"
            ),
            None,
            "⚠⚠⚠ an echo and a box rule are not a report",
        );
        assert_eq!(
            said(&[], "anything"),
            None,
            "⚠ and neither is nothing at all"
        );
    }
}
