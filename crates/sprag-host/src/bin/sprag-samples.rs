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

    println!("{} rows in {}", log.runs.len(), path.display());
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
        let said = counts
            .iter()
            .map(|(arm, count)| format!("{} {count}", arm.word()))
            .collect::<Vec<_>>()
            .join("  ");
        println!("  {:18} {said}", tally.word());
    }
    // ⚠⚠⚠ AND THE SUM IS PRINTED AS A CHECK A READER CAN DO — nothing here can be unclassified,
    // so a total that does not match the row count is this tool disagreeing with itself rather
    // than a population somebody has to interpret.
    println!(
        "  {:18} every row is in exactly one arm, so each line sums to {}",
        "",
        log.runs.len()
    );
    std::process::ExitCode::SUCCESS
}
