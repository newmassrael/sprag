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
use sprag_host::runs::{RunLog, Sampled, Tally};

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
    use super::{Reading, RunLog, lines};

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
