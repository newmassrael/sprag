//! **WHAT ONE PROCESS ASKED A PSEUDOTERMINAL NAMESPACE FOR, WITHOUT WAITING TO BE REFUSED** —
//! register item 817.
//!
//! Run this AFTER a suite that was told where to record, and it prints the demand of the largest
//! single process in that run.
//!
//! ```text
//! SPRAG_PTY_DEMAND=/tmp/demand cargo test --workspace --no-fail-fast
//! cargo run -q -p sprag-gate --bin pty-demand -- /tmp/demand
//! ```
//!
//! # ⛔⛔⛔ Why the number was debt until this existed
//!
//! `sprag_terminal::pty` has counted this since register item 814 and could only ever say it in a
//! REFUSAL — the one moment it is too late to act on. On the platform where the refusals happen
//! there is no `strace` to take it any other way, so the count that diagnosed item 814 had to be
//! taken on Linux, on another machine, and nothing in this repository could repeat it.
//!
//! # ⚠⚠ It reports and does not yet refuse a size
//!
//! Item 817's done-when (1) is that the demand becomes a MEASURED predicate; (2) and (3) — getting
//! it under the smallest namespace this product runs on, and a gate that refuses growth — need a
//! threshold, and a threshold picked before the first honest measurement is a number somebody
//! keeps. What this DOES refuse is a reading that was not made: an absent directory, an empty one,
//! a file it cannot parse, and a file with fewer lines than the count it claims. See
//! [`sprag_gate::pty_demand`] for why not one of those is a demand of zero.

fn main() -> std::process::ExitCode {
    let mut args = std::env::args_os().skip(1);
    let Some(dir) = args.next() else {
        eprintln!(
            "pty-demand: say which directory the suite recorded into — the same path its \
             SPRAG_PTY_DEMAND named"
        );
        return std::process::ExitCode::FAILURE;
    };
    if let Some(extra) = args.next() {
        eprintln!(
            "pty-demand: one directory, and {extra:?} is a second. A run records every process \
             into one place, and reading two would answer about neither"
        );
        return std::process::ExitCode::FAILURE;
    }

    let demands = match sprag_gate::pty_demand::demands(std::path::Path::new(&dir)) {
        Ok(demands) => demands,
        Err(unread) => {
            eprintln!("pty-demand: {unread}");
            return std::process::ExitCode::FAILURE;
        }
    };

    let Some(peak) = demands.first() else {
        // `demands` refuses an empty reading, so this is unreachable — said rather than
        // `unwrap`ped, because the arm that cannot happen is the one that happens.
        eprintln!("pty-demand: a reading with no process in it got past the refusal above");
        return std::process::ExitCode::FAILURE;
    };
    println!(
        "pty-demand: {} process(es) recorded; the largest took {} pseudoterminal(s)",
        demands.len(),
        peak.opened,
    );
    // Every process, largest first: the SHAPE is what item 817 is about — one arm taking hundreds
    // is a different repair from a thousand tests taking one each.
    for process in &demands {
        println!(
            "  pid {}: took {} over {} recorded line(s)",
            process.pid, process.opened, process.takes,
        );
    }
    std::process::ExitCode::SUCCESS
}
