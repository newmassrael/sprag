//! 🎯🎯🎯🎯🎯 **HOW OFTEN A TEST HAS ACTUALLY FAILED, ASKED OF THIS TREE'S OWN ARCHIVE** —
//! register item 1109.
//!
//! ```text
//! cargo run -q -p sprag-gate --bin flake-rate -- a_person_at_a_real_keyboard_who_is_not_waited_for_keeps_the_pane
//! cargo run -q -p sprag-gate --bin flake-rate -- --logs target/bx-logs --since 20260903 <test>…
//! ```
//!
//! # ⛔⛔⛔⛔⛔ What this exists for: a register item built out of anecdotes
//!
//! Every run this workspace makes leaves a log under `target/bx-logs/`, and until this binary
//! **nothing had ever read one as data** — `bx-logs` appeared in the tree only inside prose.
//! Register item 683 named a CLASS of tests that *"shake under the sweep's load"*, and it was
//! assembled from the two or three reds somebody happened to be watching. Asked of the archive on
//! 2026-09-15, its three members had **3,146 recorded outcomes**, and the claim is refuted by the
//! record it was drawn from: before 2026-09-03 the BUSY band failed at **2.33%** and the quiet band
//! at **4.05%**. The quiet band failed more often.
//!
//! ⚠⚠ **That took one afternoon and a throwaway script, which is why it had never been done.** The
//! measurement is not hard; it was unreachable. This makes it a command, so *how often does it
//! really shake* can be put to the record on the day a claim is made — and the ledger's own rule
//! that **«플레이크» is the name of having stopped diagnosing** has something to reach for.
//!
//! # ⚠⚠⚠ Why it exits 0 whatever it finds
//!
//! It is a READING and not a gate. The archive is this machine's — a clone has none — so a
//! non-zero here would be red everywhere but on the box that happens to have run the tests, which
//! is [`sprag_gate::north_star`]'s own argument about a table of readings. A rate this prints is
//! evidence a round puts in the register; it is not a verdict about the tree.
//!
//! ⚠ It exits non-zero for exactly one thing: a request it could not carry out — no test named, or
//! a log directory it cannot read. *I could not look* and *I looked and found nothing* are
//! different answers, and register item 709's discipline is that no caller may fill in the first.

/// Where the logs live, relative to the working directory, unless `--logs` says otherwise.
const ARCHIVE: &str = "target/bx-logs";

/// The flag naming a different archive.
const LOGS: &str = "--logs";

/// The flag naming the earliest log to read, as the `YYYYMMDD` a log's own filename starts with.
///
/// ⚠⚠ **THE DATE COMES OFF THE FILENAME AND NEVER OFF A CLOCK.** This binary reads no clock at all:
/// `bx` stamps every log `<YYYYMMDDTHHMMSSZ>-<label>.log`, so the name IS the record of when the
/// run happened, and a modification time would be a fact about the filesystem instead. A band split
/// on the wrong side of that difference is how *this stopped happening* becomes *somebody touched
/// the files*.
const SINCE: &str = "--since";

/// The thread count at or above which a run counts as BUSY.
///
/// ⚠ Item 683's own readings put its members' reds at `RUST_TEST_THREADS` 26 and 31 and called the
/// runner busy, so the split is drawn where that item drew it — the point is to answer that item in
/// its own terms rather than to pick a better boundary and talk past it. A caller who wants another
/// one says `--busy`.
const BUSY_DEFAULT: u32 = 26;

/// The flag naming a different busy threshold.
const BUSY: &str = "--busy";

fn main() -> std::process::ExitCode {
    let mut archive = std::path::PathBuf::from(ARCHIVE);
    let mut since: Option<String> = None;
    let mut busy = BUSY_DEFAULT;
    let mut wanted: Vec<String> = Vec::new();
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            LOGS => match args.next() {
                Some(dir) => archive = std::path::PathBuf::from(dir),
                None => return refused(&format!("{LOGS} needs a directory")),
            },
            SINCE => match args.next() {
                Some(day) => since = Some(day),
                None => return refused(&format!("{SINCE} needs a YYYYMMDD")),
            },
            BUSY => match args.next().and_then(|n| n.parse().ok()) {
                Some(threads) => busy = threads,
                None => return refused(&format!("{BUSY} needs a thread count")),
            },
            _ => wanted.push(arg),
        }
    }
    if wanted.is_empty() {
        return refused("name at least one test");
    }
    let Ok(entries) = std::fs::read_dir(&archive) else {
        return refused(&format!(
            "cannot read {} — name another with {LOGS}",
            archive.display()
        ));
    };

    // ⚠⚠ SORTED, so a reader can see the window this walked rather than the order a filesystem
    // happened to hand back. `bx`'s names begin with the stamp, so the sort is chronological.
    let mut logs: Vec<std::path::PathBuf> = entries
        .filter_map(|entry| Some(entry.ok()?.path()))
        .filter(|path| path.extension().is_some_and(|kind| kind == "log"))
        .collect();
    logs.sort();

    let mut counted = 0_usize;
    let mut tally: Vec<(
        String,
        sprag_gate::sweep::Outcomes,
        sprag_gate::sweep::Outcomes,
    )> = wanted
        .iter()
        .map(|test| {
            (
                test.clone(),
                sprag_gate::sweep::Outcomes::default(),
                sprag_gate::sweep::Outcomes::default(),
            )
        })
        .collect();
    for path in &logs {
        let named = path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        if since
            .as_ref()
            .is_some_and(|day| named.as_str() < day.as_str())
        {
            continue;
        }
        // ⚠ A log this cannot read is SKIPPED and counted out of the window rather than guessed at
        // — a run whose bytes are gone is not a run that passed.
        let Ok(text) = std::fs::read_to_string(path) else {
            continue;
        };
        counted += 1;
        // ⚠⚠ A log that does not say how many threads it was given goes in the QUIET band's
        // company only by never being counted in either: see `threads_in`, whose `None` must not
        // be read as one thread. It still counts toward the total.
        let band = sprag_gate::sweep::threads_in(&text);
        for (test, all, hot) in &mut tally {
            let seen = sprag_gate::sweep::outcomes_of(test, &text);
            *all = all.and(seen);
            if band.is_some_and(|threads| threads >= busy) {
                *hot = hot.and(seen);
            }
        }
    }

    println!(
        "read {counted} log(s) under {}{}",
        archive.display(),
        since.map_or_else(String::new, |day| format!(", from {day} onward")),
    );
    for (test, all, hot) in &tally {
        // ⚠⚠⚠ THE TWO EMPTIES ARE SAID APART. *Never seen* and *seen and never red* are opposite
        // findings about a claim of flakiness, and a rate of `0` for the first is the silence that
        // let item 683 stand for three weeks.
        let rate = all
            .per_myriad()
            .map_or_else(|| "never ran".to_owned(), |per| format!("{per} per 10k"));
        let of_busy = hot
            .per_myriad()
            .map_or_else(|| "never ran".to_owned(), |per| format!("{per} per 10k"));
        println!(
            "{test}: {} ok, {} FAILED, {rate} — at {BUSY} >= {busy}: {} ok, {} FAILED, {of_busy}",
            all.passed, all.failed, hot.passed, hot.failed,
        );
    }
    std::process::ExitCode::SUCCESS
}

/// Say what could not be carried out, and exit non-zero — the one ending that is not a reading.
fn refused(why: &str) -> std::process::ExitCode {
    eprintln!("flake-rate: {why}");
    std::process::ExitCode::FAILURE
}
