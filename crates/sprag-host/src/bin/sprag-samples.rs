//! ⛔⛔⛔⛔⛔ **WHICH RUNS A RATE MAY BE TAKEN OVER** — register item 895, and the mouth that
//! predicate never had.
//!
//! ## Why this is a tool and not a library call nobody makes
//!
//! Every measurement of the run store has been a `python3 -c` typed into a round. Measured
//! 2026-09-05: nothing under `crates/` reads `*.runs.json` for analysis at all — the file's only
//! product reader is `sprag_host::durability`, which restores from it into a daemon. So the
//! population question has been answered by a fresh filter each time, and
//! [`sprag_host::runs::Sampled`]'s own doc records four of those filters disagreeing:
//! two counts of one population came out **8 against 10**, both right about their own predicate.
//!
//! ⇒ A number a reader cannot attach a predicate to is not a measurement. This prints the
//! partition with the predicate's own words on it, so a round quotes `counted 11` and the word
//! says what was counted.
//!
//! ## Why it needs no promotion, which is the whole reason it is shaped this way
//!
//! It answers AT READ TIME from a file, so it says something true about runs that ended under any
//! build — including the ones a live daemon predates. Register item 868's ceiling (*a promotion is
//! the upper bound on instrumentation*) reaches an instrument that has to RUN in production; the
//! narrowing item 872 recorded is that a reader-time instrument escapes it. This is that shape.
//!
//! ## What it will not do
//!
//! It prints no rate. `zeroed` is undecidable per row for everything the store already holds (see
//! [`sprag_host::runs::Sampled::Zeroed`]), so a ratio printed here would be this
//! tool choosing the very thing item 895 exists to stop being chosen silently. The three counts go
//! out beside each other and the reader decides, in writing.

use sprag_host::moment::Reading;
use sprag_host::runs::{AcrossRows, RunLog, Sampled, Tally};

fn main() -> std::process::ExitCode {
    let mut args = std::env::args_os().skip(1);
    let Some(path) = args.next() else {
        eprintln!(
            "sprag-samples: needs the path of a run store — \
             $XDG_STATE_HOME/sprag/<socket>.runs.json"
        );
        return std::process::ExitCode::FAILURE;
    };
    if args.next().is_some() {
        eprintln!("sprag-samples: takes one run store and nothing else");
        return std::process::ExitCode::FAILURE;
    }
    let path = std::path::PathBuf::from(path);
    // ⛔⛔⛔⛔⛔ THE CLOCK IS READ WHERE THE BYTES ARE — register item 918. This file is one a
    // daemon is still appending to, so running this command again is a NEW measurement rather than
    // a check of the last one, and a stamp taken at print time would say when the tool got round
    // to speaking. Item 895 paid for the difference: three readings of one ratio (0.3130, 0.3114,
    // 0.3036) that a later round read as a contradiction because the first carried no moment.
    let at = sprag_host::moment::Reading::now();
    let read = match std::fs::read_to_string(&path) {
        Ok(read) => read,
        Err(why) => {
            eprintln!("sprag-samples: cannot read {}: {why}", path.display());
            return std::process::ExitCode::FAILURE;
        }
    };
    // ⚠ THE PRODUCT'S OWN DECODE, never a hand-walked `serde_json::Value` — the point of this tool
    // is that the population is asked of the record rather than of the file's shape, and item 891's
    // addendum measured why: the store re-serialises every row through the CURRENT struct on every
    // save, so a key's presence in the text is retroactive and says nothing about the build.
    let log: RunLog = match serde_json::from_str(&read) {
        Ok(log) => log,
        Err(why) => {
            eprintln!(
                "sprag-samples: {} is not a run store: {why}",
                path.display()
            );
            return std::process::ExitCode::FAILURE;
        }
    };

    for line in lines(&log, &path, at) {
        println!("{line}");
    }
    std::process::ExitCode::SUCCESS
}

/// This tool's BODY, separated from the printing so a gate can read what it says — `sprag`'s own
/// `folds_lines` split, for its reason: register item 856 ⑸ measured a value crossing into a mouth
/// and vanishing there while every library gate stayed green.
fn lines(log: &RunLog, path: &std::path::Path, at: Reading) -> Vec<String> {
    // ⛔⛔⛔⛔⛔ AND WHEN IT WAS READ, ON THE LINE THAT SAYS HOW MANY — register item 918. The
    // count and the moment are one fact about a live file: `245 rows` is true of an instant, and
    // the register's own rule is that a number carries the moment it was read because the command
    // takes a new measurement rather than checking the old one.
    let mut said = vec![format!(
        "{} rows in {}  read at {at}",
        log.runs.len(),
        path.display()
    )];
    for tally in Tally::ALL {
        // ⚠⚠ EVERY ARM IS PRINTED, INCLUDING A ZERO — this workspace's rule 6. An arm left out
        // because it happened to be empty is exactly how `unsaid` would stop being a word a reader
        // knows to ask for, and it is zero today for every row the store already held.
        let counts = Sampled::ALL.map(|arm| {
            (
                arm,
                log.runs
                    .iter()
                    .filter(|run| run.sampled(tally) == arm)
                    .count(),
            )
        });
        let counted = counts
            .iter()
            .map(|(arm, count)| format!("{} {count}", arm.word()))
            .collect::<Vec<_>>()
            .join("  ");
        said.push(format!("  {:18} {counted}", tally.word()));
        // 🎯🎯🎯 AND THE READING THE COLUMN'S OWN DOC INSTRUCTS, for the one kind of column where
        // the three arms above do not carry it — register item 962.
        //
        // ⛔⛔ The arms answer *how many rows had a non-zero depth*. `reask_landed_deepest` exists
        // to answer *how deep did the deepest go*, and those are different questions: twenty
        // landings on FIRST asks are no evidence at all for a bound of two. Until this line, the
        // second question could only be put by writing a filter over the store file by hand —
        // which is the disease `Sampled` was built to end.
        //
        // ⚠ Driven off `Tally::across_rows` rather than by naming the column, so a second maximum
        // added tomorrow gets a mouth here or fails the gate in this file.
        match tally.across_rows() {
            AcrossRows::Maximum => {
                said.push(format!(
                    "  {:18} {}",
                    "",
                    log.deepest_reask_landing().describe()
                ));
            }
            AcrossRows::Total | AcrossRows::Table => {}
        }
    }
    // ⚠⚠⚠ AND THE SUM IS PRINTED AS A CHECK A READER CAN DO — nothing here can be unclassified,
    // so a total that does not match the row count is this tool disagreeing with itself rather
    // than a population somebody has to interpret.
    said.push(format!(
        "  {:18} every row is in exactly one arm, so each line sums to {}",
        "",
        log.runs.len()
    ));
    said
}

#[cfg(test)]
mod tests {
    use super::{AcrossRows, Reading, RunLog, Tally, lines};

    /// A store of `depths`, each a row whose `reask_landed_deepest` is that value — `None` for a
    /// row from a build that never carried the column.
    fn store_of(depths: &[Option<u32>]) -> RunLog {
        let runs: Vec<serde_json::Value> = depths
            .iter()
            .enumerate()
            .map(|(at, depth)| {
                let mut row = serde_json::json!({
                    "id": at + 1, "label": "ai_loop pane=1", "iterations": 1, "finished": true,
                });
                if let Some(depth) = depth {
                    row["reask_landed_deepest"] = serde_json::json!(depth);
                }
                row
            })
            .collect();
        serde_json::from_value(serde_json::json!({
            "version": sprag_host::runs::RUN_LOG_VERSION,
            "runs": runs,
        }))
        .expect("the log a predecessor leaves is what this reads")
    }

    /// What this tool printed for `depths`, as one string.
    fn page(depths: &[Option<u32>]) -> String {
        lines(
            &store_of(depths),
            std::path::Path::new("/tmp/one.runs.json"),
            Reading::at(1_788_681_668),
        )
        .join("\n")
    }

    /// 🎯🎯🎯🎯🎯 **THE READING THE COLUMN'S DOC INSTRUCTS COMES OUT OF THIS COMMAND** — register
    /// item 962, and its `Done when` ⑵.
    ///
    /// # ⛔⛔⛔ The mutation this is built to catch
    ///
    /// *Lower the maximum and it must go red.* A reader that answered the LAST row, or the first,
    /// or the count of non-zero rows — which is what the three `Sampled` arms already say — passes
    /// a page that merely mentions a number. So the rows are ordered with the deepest in the
    /// MIDDLE: last-wins answers 2, first-wins answers 1, counting answers 3, and only a maximum
    /// answers 7.
    #[test]
    fn the_page_says_the_deepest_any_row_reached_and_not_how_many_reached_one() {
        let said = page(&[Some(1), Some(7), Some(2)]);
        assert!(
            said.contains("the deepest any row reached is 7"),
            "⛔ ITEM 962: `reask_landed_deepest` is a MAXIMUM, and the three sampled arms answer \
             *how many rows had one*. Lowering it to any row but the deepest is the mutation this \
             arm exists for. Got:\n{said}",
        );
        // ⚠⚠ AND THE POPULATION TRAVELS WITH IT — a maximum over a population nobody stated is the
        // number item 895 spent four readers proving is not a measurement.
        assert!(
            said.contains("over 3 row(s) that carried one"),
            "⚠ the maximum must say what it was taken over. Got:\n{said}",
        );
        // ⛔ AND THE DIRECTION, because the column's doc says this answers *lowered to* and never
        // *raised to* — a censored deeper landing is not an absent one, and that sentence is the
        // whole reason the number is safe to act on.
        assert!(
            said.contains("lowered to this") && said.contains("never"),
            "⚠⚠ the direction is part of the reading, not a caveat kept elsewhere. Got:\n{said}",
        );
    }

    /// ⛔⛔⛔⛔⛔ **AN EMPTY POPULATION SAYS SO AND DOES NOT SAY ZERO** — register item 962, and the
    /// case the live store is actually in.
    ///
    /// Measured 2026-09-08 over the loop's own file: **261 rows and `unsaid` on every one** — the
    /// daemon that wrote them predates the column. A page printing `0` there would be read as
    /// *every ask-again landed on the first try*, which is the opposite of what the file says, and
    /// it is register item 924's shape: a number that is green because nothing was in its
    /// population.
    #[test]
    fn a_store_where_nobody_recorded_a_depth_says_that_rather_than_zero() {
        let said = page(&[None, None]);
        assert!(
            said.contains("no row carries a depth") && said.contains("this is not a depth of 0"),
            "⛔ ITEM 962/924: an empty population must SAY it is empty. Got:\n{said}",
        );
        // ⚠ And a genuine zero is a different page — `Some(0)` is *counted and found none*, which
        // item 891 put a whole third arm into `Sampled` to keep apart from *nobody counted*.
        let counted = page(&[Some(0)]);
        assert!(
            counted.contains("the deepest any row reached is 0"),
            "⚠⚠ a recorded zero is a reading, not an absence — the two must not share a page. \
             Got:\n{counted}",
        );
    }

    /// ⚠⚠ **EVERY COLUMN READ AS A MAXIMUM HAS A MOUTH HERE** — the arm that stops a second one
    /// added tomorrow from being printed as three row-counts and nothing else.
    ///
    /// ⚠ It walks `Tally::ALL` rather than naming the column, so the population is the enum's and
    /// not a list kept in this file.
    #[test]
    fn every_maximum_column_is_printed_as_a_maximum() {
        let said = page(&[Some(4)]);
        let maxima: Vec<Tally> = Tally::ALL
            .into_iter()
            .filter(|tally| tally.across_rows() == AcrossRows::Maximum)
            .collect();
        assert!(
            !maxima.is_empty(),
            "⛔ no column is classified as a maximum, so this gate has an empty population and \
             passes by reading nothing — `Tally::across_rows` is where that is decided",
        );
        for tally in maxima {
            assert!(
                said.contains(tally.word()),
                "⚠ {} is read as a maximum and this page never names it. Got:\n{said}",
                tally.word(),
            );
        }
        assert!(
            said.contains("the deepest any row reached is 4"),
            "⚠⚠ naming the column is not printing its reading. Got:\n{said}",
        );
    }

    /// ⛔⛔⛔⛔⛔ **THE PARTITION IS PRINTED BESIDE THE MOMENT IT WAS READ** — register item 918,
    /// and the third mouth of the three this workspace publishes live-store numbers through.
    ///
    /// # ⛔⛔⛔ Why a mouth needs its own gate when the moment is already a type
    ///
    /// [`sprag_host::moment::Reading`] renders correctly and is gated where it lives. Item 856 ⑸
    /// measured what that is not enough for: a value that passes seven gated surfaces and is
    /// DISCARDED by the call that would put it on the page leaves the whole workspace green. This
    /// tool's `println!` is that call, and before item 918 there was nothing here to discard.
    #[test]
    fn the_partition_says_when_the_store_was_read() {
        let log: RunLog = serde_json::from_value(serde_json::json!({
            "version": sprag_host::runs::RUN_LOG_VERSION,
            "runs": [
                // One row of each answer, so the moment is not the only thing on the page: a row
                // with a counted split, one present-and-all-zero, and one from a build older than
                // the table at all.
                {"id": 1, "label": "ai_loop pane=1", "iterations": 1, "finished": true,
                 "deliveries": {"made": 4, "folded": 1},
                 "folds_by_reason": {"ordinary": {"delivered": 4, "folded": 1}}},
                {"id": 2, "label": "ai_loop pane=2", "iterations": 1, "finished": true,
                 "deliveries": {"made": 3, "folded": 0},
                 "folds_by_reason": {"ordinary": {"delivered": 0, "folded": 0}}},
                {"id": 3, "label": "ai_loop pane=3", "iterations": 1, "finished": true},
            ]
        }))
        .expect("the log a predecessor leaves is what this reads");
        // ⚠ A NAMED moment and never `Reading::now()`: the clock is the one input a gate cannot
        // state, and what this gate is about is the page SAYING it. Checked against
        // `date -u -d @1788681668`.
        let said = lines(
            &log,
            std::path::Path::new("/tmp/one.runs.json"),
            Reading::at(1_788_681_668),
        );
        assert!(
            said.first().is_some_and(
                |head| head.contains("2026-09-06T08:01:08Z") && head.contains("3 rows")
            ),
            "⛔⛔⛔⛔⛔ REGISTER ITEM 918: the row count and the moment are ONE fact about a live \
             file — `3 rows` is true of an instant, and a count printed without it cannot be told \
             from a later reading of the same store. Got: {said:?}",
        );
        // ⚠⚠ AND ON THE LINE THAT CARRIES THE COUNT, not appended at the end: a stamp under the
        // numbers it qualifies is read after they have already been quoted.
        assert!(
            said.iter()
                .filter(|line| line.contains("2026-09-06T08:01:08Z"))
                .count()
                == 1,
            "⚠⚠ ONCE, on the heading. Got: {said:?}",
        );
        // ⛔ AND THE PARTITION IS STILL THERE — a gate for the stamp that passed over an empty
        // page would be measuring nothing at all.
        assert!(
            said.iter()
                .any(|line| line.contains("folds_by_reason") && line.contains("counted 1")),
            "⚠ the page still answers the question it exists for. Got: {said:?}",
        );
    }
}
