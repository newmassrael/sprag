//! **THE READING THAT WAS NOT MADE MUST NOT LOOK LIKE A SMALL ONE** — register item 817, and the
//! rule this workspace keeps paying for: an unclassified case is RED and never a pass.
//!
//! [`sprag_gate::pty_demand`] answers *how much of a pseudoterminal namespace did the biggest
//! single process in this run ask for*, by reading what `sprag_terminal::pty` appended. Every way
//! that reading can fail to happen has a reader here, because each one would otherwise arrive as
//! the number `0` — and `0` is the one answer that makes a gate green forever.

use std::path::{Path, PathBuf};

use sprag_gate::pty_demand::{Unread, demands};

/// A directory of this test's own, named for what it holds and for this process.
///
/// ⚠ `CARGO_TARGET_TMPDIR` and not `std::env::temp_dir()` — register item 794. The bare call
/// answers a RELATIVE path when `TMPDIR` is set-and-empty, and `cargo test` stands every binary in
/// its own crate directory, so these files would land inside the repository. Cargo hands an
/// integration test an absolute one that no environment variable can bend, which is also why this
/// crate does not have to grow a dependency to ask.
fn scratch(what: &str) -> PathBuf {
    let dir =
        PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("{what}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("a scratch directory");
    dir
}

/// One process's recording, as the recorder writes it: one line per take, numbered from 1.
fn recorded(dir: &Path, pid: &str, takes: u64) {
    let body: String = (1..=takes)
        .map(|n| format!("opened={n} live=1\n"))
        .collect();
    std::fs::write(dir.join(pid), body).expect("write a process's recording");
}

/// **THE ANSWER IS ONE PROCESS'S DEMAND, NOT THE RUN'S TOTAL.**
///
/// Item 817's own note refuses the sum as the unit: processes that ran one after another never
/// held anything at the same time, so a namespace is exhausted by the LARGEST of them. A gate that
/// added these two would report 8 where the truth about any moment is 5.
#[test]
fn the_largest_process_is_the_answer_and_the_sum_is_not() {
    let dir = scratch("largest");
    recorded(&dir, "111", 5);
    recorded(&dir, "222", 3);

    let found = demands(&dir).expect("two processes recorded");
    assert_eq!(
        found.iter().map(|d| d.opened).collect::<Vec<_>>(),
        [5, 3],
        "largest first, and the reader is handed both so the SHAPE is visible: one arm taking \
         hundreds is a different repair from a thousand tests taking one each",
    );
    assert_eq!(
        found[0].pid, "111",
        "a row is named by the process that wrote it"
    );
    assert_eq!(found[0].takes, 5, "one line per take");
    std::fs::remove_dir_all(&dir).expect("clean up");
}

/// **A DIRECTORY THAT IS NOT THERE IS NOT A SUITE THAT WANTED NOTHING.**
///
/// The instrument was off, was pointed somewhere else, or could not write — three repairs, and the
/// number `0` names none of them.
#[test]
fn an_absent_directory_is_refused_and_never_read_as_zero() {
    let missing =
        PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("absent-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&missing);
    assert!(
        matches!(demands(&missing), Err(Unread::NoDirectory { .. })),
        "⛔⛔⛔⛔⛔ REGISTER ITEM 817: a reading nobody could take came back as an answer",
    );
}

/// **AND NEITHER IS AN EMPTY ONE** — the shape a broken recorder actually leaves behind, since the
/// directory is created by whoever set the variable and the files by whoever opened a pane.
#[test]
fn a_directory_no_process_wrote_to_is_refused() {
    let dir = scratch("empty");
    assert!(
        matches!(demands(&dir), Err(Unread::NoProcess { .. })),
        "⛔⛔⛔⛔⛔ REGISTER ITEM 817: a suite that drives panes takes pseudoterminals, so an empty \
         directory says the recording did not happen — not that none were wanted",
    );
    std::fs::remove_dir_all(&dir).expect("clean up");
}

/// **A FILE WITH NO LINE IN IT IS A RECORDER THAT STOPPED**, which is not a process that took
/// nothing: the file is created by the first take.
#[test]
fn a_file_with_nothing_in_it_is_refused() {
    let dir = scratch("silent");
    std::fs::write(dir.join("333"), "").expect("an empty recording");
    assert!(
        matches!(demands(&dir), Err(Unread::Silent { .. })),
        "⛔⛔⛔ REGISTER ITEM 817: an empty file read as a process that asked for nothing",
    );
    std::fs::remove_dir_all(&dir).expect("clean up");
}

/// **AN UNREADABLE LINE IS REFUSED RATHER THAN SKIPPED.**
///
/// Skipping is the failure that hides: a gate that ignored what it could not parse would
/// UNDER-REPORT the demand by exactly the lines a format change made unreadable, and report a
/// smaller number with no sign that anything was wrong.
#[test]
fn a_line_this_gate_cannot_read_is_refused_rather_than_skipped() {
    let dir = scratch("unparsable");
    std::fs::write(dir.join("444"), "opened=1 live=0\ntook one more\n").expect("a changed format");
    assert!(
        matches!(demands(&dir), Err(Unread::Unparsable { .. })),
        "⛔⛔⛔⛔⛔ REGISTER ITEM 817: the unreadable line was skipped, so the answer was the \
         readable half of a recording and said so nowhere",
    );
    std::fs::remove_dir_all(&dir).expect("clean up");
}

/// **AND SO IS HALF THE FORM.** The `live=` half is required even though nothing reads its value:
/// what this gate accepts has to be what the recorder writes, or a format that lost a field goes
/// through unnoticed.
#[test]
fn a_line_missing_the_half_nobody_reads_is_still_refused() {
    let dir = scratch("halved");
    std::fs::write(dir.join("555"), "opened=1\n").expect("a recording that lost a field");
    assert!(
        matches!(demands(&dir), Err(Unread::Unparsable { .. })),
        "⛔⛔ REGISTER ITEM 817: a line the recorder does not write was accepted",
    );
    std::fs::remove_dir_all(&dir).expect("clean up");
}

/// **FEWER LINES THAN TAKES MEANS WRITES WERE LOST**, and then every number in the file is a floor
/// rather than a reading.
///
/// ⚠ The OTHER direction is ordinary and must stay green: a process can hold a second ledger of
/// its own — `sprag_terminal::pty`'s own unit test makes one, numbering from 1 in the same file —
/// so more lines than the peak is a thing this gate expects to see.
#[test]
fn a_recording_shorter_than_the_count_it_claims_is_refused() {
    let dir = scratch("lost");
    std::fs::write(dir.join("666"), "opened=1 live=1\nopened=9 live=1\n").expect("a short file");
    assert!(
        matches!(demands(&dir), Err(Unread::Lost { .. })),
        "⛔⛔⛔ REGISTER ITEM 817: nine takes left two lines and the gate reported nine",
    );

    let more = scratch("second-ledger");
    std::fs::write(
        more.join("777"),
        "opened=1 live=1\nopened=2 live=1\nopened=1 live=1\n",
    )
    .expect("a file two ledgers wrote");
    let found = demands(&more).expect("two ledgers in one process is ordinary");
    assert_eq!(
        (found[0].opened, found[0].takes),
        (2, 3),
        "the peak is the demand and the extra line is not an error",
    );
    std::fs::remove_dir_all(&dir).expect("clean up");
    std::fs::remove_dir_all(&more).expect("clean up");
}
