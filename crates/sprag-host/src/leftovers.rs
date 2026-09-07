//! ⛔⛔⛔⛔⛔ **WHAT A DEAD DAEMON LEFT IN THE STATE DIRECTORY, AND WHETHER IT MAY GO** — register
//! item 905.
//!
//! # ⛔⛔⛔⛔⛔ Every TUI client mints a socket, and every socket mints a state file that outlives it
//!
//! A client that opens its own daemon gets its own socket, and the daemon keys its persistent files
//! on that socket's stem. The client dies; the files stay. Measured over the loop's own state
//! directory at **2026-09-06T09:22:21Z**: **69 `*.runs.json`, 68 of them holding no run at all**,
//! the oldest dated 8-31 — the same census the ledger took on 2026-09-05, re-taken, because a
//! number over a live directory is re-measured and never re-derived.
//!
//! ⇒ It costs almost no disk. What it costs is READABILITY: a sweep of that directory walks 68
//! empty files to reach the one with anything in it, and item 905 was found because `sprag waits`
//! printed 68 blank rows above its answer.
//!
//! # ⛔⛔⛔⛔⛔ Why the unit is the STEM and not the run log — and one row today proves it
//!
//! Item 905's own done-when says *a log holding no runs disappears*. Taken literally that rule
//! **destroys a session tree**. A daemon keys THREE artefacts on one stem — the run log, the
//! workspace snapshot, and a `.history/` directory of each pane's scrollback — and *the run log is
//! empty* says nothing about the other two.
//!
//! Measured at **2026-09-06T09:24:08Z**, the pane counts across those 69 snapshots were
//! `{0: 67, 1: 1, 7: 1}`:
//!
//! | stem | socket | runs | panes |
//! |---|---|---:|---:|
//! | `sprag-loop` | answers | 246 | 7 |
//! | `sprag-tui-pty-910212-22` | gone | **0** | **1** |
//! | 67 others | gone | 0 | 0 |
//!
//! ⇒ ⭐ **The middle row is the whole item.** Its run log is empty, so the done-when as written
//! would remove it — along with a snapshot holding a pane and a `.history/0.hist` holding that
//! pane's scrollback, which a successor daemon on that socket restores from.
//!
//! # ⚠⚠ And *no runs* and *no daemon* coincide today, which is exactly why neither may be the test
//!
//! At **2026-09-06T09:23:37Z** exactly one stem of the 69 had a socket file, and it was the same
//! stem as the only one holding runs. A predicate resting on either fact alone is right about every
//! row in this directory today and wrong the first time a daemon dies with records in it. So both
//! are asked, the answers are a closed vocabulary, and the combinations nobody has produced yet are
//! arms rather than absences — this workspace's rule 6.

use std::path::{Path, PathBuf};

/// How long to wait for a socket to answer before calling it silent.
///
/// ⚠ Short on purpose: this is a census of a directory, run from a person's terminal, and a stem
/// whose socket is gone is the COMMON case — 68 of 69. A generous timeout here is 68 multiples of
/// itself spent proving what the missing file already said.
const KNOCK: std::time::Duration = std::time::Duration::from_millis(250);

/// ⛔⛔⛔⛔⛔ **WHAT ONE DAEMON'S RESIDUE IS, AND THEREFORE WHETHER IT MAY BE REMOVED** — register
/// item 905, and a closed vocabulary because every arm is a different refusal.
///
/// ⚠⚠ **ONLY [`Spent`](Self::Spent) MAY GO.** Every other arm is a reason to keep, and they are
/// kept apart rather than folded into *no* because a person who is told *no* still has to know
/// whether to stop a daemon, read the records, or restore the panes — and because folding them
/// would let a later edit widen one of them by accident.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LeftBehind {
    /// **A DAEMON ANSWERED ON THIS STEM'S SOCKET.** It is writing to these files now.
    Serving,
    /// ⛔ **SOMETHING IS LISTENING AND WOULD NOT TALK** — `sprag_rpc::survey::Answered::Refused`,
    /// carrying the product's own sentence.
    ///
    /// ⚠⚠ IT IS A KEEP, and that is the direction that matters: a daemon whose wire this build
    /// cannot speak is still a daemon, and its files are still being written. Reading *not a
    /// daemon I recognise* as *nobody is there* is how a sweep would delete a running loop's
    /// records — the exact confusion `Answered` exists to prevent one level down.
    Listening(String),
    /// **NOTHING IS LISTENING AND THE RUN LOG HOLDS RUNS.** Kept: this is the record `sprag folds`,
    /// `sprag waits` and `sprag-samples` are read from, and register items 892 and 918 are about
    /// quoting numbers out of it years later.
    HoldsRuns(usize),
    /// ⛔⛔⛔ **NOTHING IS LISTENING, NO RUNS, AND THE SNAPSHOT HOLDS PANES.** Kept, and this is the
    /// arm item 905's own done-when would have deleted: `sprag-tui-pty-910212-22` at
    /// 2026-09-06T09:24:08Z — 0 runs, 1 pane, and a `.history/0.hist` beside it.
    HoldsPanes(usize),
    /// 🎯 **NOTHING IS LISTENING, NO RUNS, NO PANES.** The only arm that may be removed.
    Spent,
    /// ⛔⛔⛔⛔⛔ **THIS BUILD CANNOT READ ONE OF THE FILES.** Kept, and said out loud — this
    /// workspace's rule 6, and the sharpest case of it here: *I could not read it* must never
    /// become *there was nothing in it*. A log written by a newer build, or half-written by a
    /// crash, lands here.
    Unreadable(String),
}

impl LeftBehind {
    /// The word a row is keyed by — one token, so a caller may branch on it without reading the
    /// sentence beside it. ⛔ An exhaustive `match` with no `_`.
    #[must_use]
    pub const fn word(&self) -> &'static str {
        match self {
            Self::Serving => "serving",
            Self::Listening(_) => "listening",
            Self::HoldsRuns(_) => "holds-runs",
            Self::HoldsPanes(_) => "holds-panes",
            Self::Spent => "spent",
            Self::Unreadable(_) => "unreadable",
        }
    }

    /// 🎯🎯🎯🎯🎯 **WHETHER THIS RESIDUE MAY BE DELETED** — register item 905's whole content, in
    /// one place so no caller may spell a second rule.
    ///
    /// ⚠⚠ It is `true` for exactly ONE arm. Written as an exhaustive match rather than as
    /// `matches!(self, Self::Spent)` so a seventh arm added above is a compile error here until
    /// somebody decides, rather than silently joining the keeps — which would be the safe
    /// direction this time and the wrong habit every other time.
    #[must_use]
    pub const fn may_remove(&self) -> bool {
        match self {
            Self::Spent => true,
            Self::Serving
            | Self::Listening(_)
            | Self::HoldsRuns(_)
            | Self::HoldsPanes(_)
            | Self::Unreadable(_) => false,
        }
    }

    /// What a reader is looking at, in one clause — ⛔ an exhaustive `match` with no `_`.
    #[must_use]
    pub fn describe(&self) -> String {
        match self {
            Self::Serving => {
                "a daemon answers on its socket and is writing to these files now".to_owned()
            }
            Self::Listening(why) => format!(
                "something is listening on its socket and would not talk, so a daemon may be \
                 writing to these files: {why}"
            ),
            Self::HoldsRuns(runs) => format!(
                "nothing is listening, and its run log holds {runs} run(s) — the record `sprag \
                 folds`, `sprag waits` and `sprag-samples` are read from"
            ),
            Self::HoldsPanes(panes) => format!(
                "nothing is listening and it logged no run, but its snapshot holds {panes} pane(s) \
                 that a successor daemon on that socket would restore"
            ),
            Self::Spent => {
                "nothing is listening, it logged no run and its snapshot holds no pane".to_owned()
            }
            Self::Unreadable(why) => format!(
                "this build cannot read one of its files, so nothing here may claim it is empty: \
                 {why}"
            ),
        }
    }
}

/// ⛔⛔⛔⛔⛔ **ONE DAEMON'S RESIDUE**, keyed on the socket stem the daemon derived all of it from.
///
/// ⚠⚠ THREE ARTEFACTS AND NOT ONE. `crate::durability` keys `runs_path`, `snapshot_path` and the
/// pane history directory on `socket_key`, so they live and die together — and item 905's own
/// done-when named only the first, which is what would have cost a session tree.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Residue {
    /// The socket stem, which is the name all three artefacts are keyed on.
    pub stem: String,
    /// What it is, and therefore whether it may go.
    pub what: LeftBehind,
    /// The files and directories that belong to this stem AND EXIST, in a stable order.
    ///
    /// ⚠ Only the ones that exist: a stem with no `.history/` is the common case, and listing a
    /// path that is not there would have a reader looking for it.
    pub holds: Vec<PathBuf>,
}

/// ⛔⛔⛔⛔⛔ **EVERY STEM THE STATE DIRECTORY HOLDS, AND WHAT EACH ONE IS** — register item 905.
///
/// `state` is the directory `crate::durability` writes to; `runtime` is where the sockets are
/// bound, which is a DIFFERENT directory — the state files outlive a reboot and the sockets do not,
/// which is the whole reason this residue exists.
///
/// ⚠⚠ The stems come from the RUN LOGS, because that is the artefact item 905 counted and the one
/// every stem here has. A snapshot with no run log beside it would be missed — measured
/// 2026-09-06T09:22:21Z, that is 0 of 69, and it is stated rather than assumed.
#[must_use]
pub fn survey(state: &Path, runtime: &Path) -> Vec<Residue> {
    let mut stems: Vec<String> = std::fs::read_dir(state)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|entry| {
            entry
                .file_name()
                .to_str()
                .and_then(|name| name.strip_suffix(".runs.json"))
                .map(str::to_owned)
        })
        .collect();
    // Sorted so two censuses of one directory are diffable, `run_logs`' own rule: the use of this
    // is comparing today's answer with one somebody wrote down.
    stems.sort();
    stems
        .into_iter()
        .map(|stem| {
            let runs = state.join(format!("{stem}.runs.json"));
            let snapshot = state.join(format!("{stem}.snapshot.json"));
            let history = state.join(format!("{stem}.history"));
            let holds = [runs.clone(), snapshot.clone(), history.clone()]
                .into_iter()
                .filter(|path| path.exists())
                .collect();
            let what = judge(&stem, &runs, &snapshot, runtime);
            Residue { stem, what, holds }
        })
        .collect()
}

/// One stem's verdict — the order of the questions IS the safety argument.
///
/// ⚠⚠⚠ **THE SOCKET IS ASKED FIRST AND UNCONDITIONALLY.** A live daemon's files may not be judged
/// on their contents at all: an empty run log on a daemon that started a minute ago is the normal
/// state of a healthy loop, and it is indistinguishable, by content, from the 67 dead ones.
fn judge(stem: &str, runs: &Path, snapshot: &Path, runtime: &Path) -> LeftBehind {
    match sprag_rpc::survey::ask(
        &runtime.join(format!("{stem}{}", sprag_rpc::survey::SOCKET_SUFFIX)),
        "sprag-leftovers",
        KNOCK,
    ) {
        sprag_rpc::survey::Answered::Serving => return LeftBehind::Serving,
        sprag_rpc::survey::Answered::Refused(why) => return LeftBehind::Listening(why),
        // ⚠ A missing socket file lands here too, and that is right: `ask` connects, and a path
        // that is not there cannot be connected to. What it means is the same — nothing this
        // machine can reach is serving that stem.
        sprag_rpc::survey::Answered::Silent => {}
    }
    // ⚠⚠ THE PRODUCT'S OWN DECODE, never a hand-walked `Value` — `sprag-samples`' rule, and for its
    // reason: the question is what the RECORD holds, and a key's presence in the text is
    // retroactive (item 891).
    let held = match read::<crate::runs::RunLog>(runs) {
        Ok(log) => log.runs.len(),
        Err(why) => return LeftBehind::Unreadable(why),
    };
    if held > 0 {
        return LeftBehind::HoldsRuns(held);
    }
    // ⚠ A stem with NO snapshot is not unreadable — it is a stem that never wrote one, which is
    // *no panes*. The distinction matters because `Unreadable` is a permanent keep.
    if !snapshot.exists() {
        return LeftBehind::Spent;
    }
    match read::<sprag_terminal::Snapshot>(snapshot) {
        Ok(snapshot) => match panes_in(&snapshot) {
            0 => LeftBehind::Spent,
            panes => LeftBehind::HoldsPanes(panes),
        },
        Err(why) => LeftBehind::Unreadable(why),
    }
}

/// Decode one of the two files, with the reason a failure would be kept for.
fn read<T: serde::de::DeserializeOwned>(path: &Path) -> Result<T, String> {
    let text = std::fs::read_to_string(path).map_err(|why| format!("{}: {why}", path.display()))?;
    serde_json::from_str(&text).map_err(|why| format!("{}: {why}", path.display()))
}

/// How many panes a snapshot would restore.
///
/// ⚠ Counted through the session tree rather than off a top-level field, because there is no
/// top-level field: a snapshot is sessions of windows of panes, and *the sessions list is not
/// empty* is a different fact — measured 2026-09-06T09:23:57Z, all 69 snapshots have a non-empty
/// `sessions` list and 67 of them hold no pane at all.
fn panes_in(snapshot: &sprag_terminal::Snapshot) -> usize {
    snapshot
        .sessions
        .iter()
        .flat_map(|session| session.windows.iter())
        .map(|window| window.panes.len())
        .sum()
}

#[cfg(test)]
mod tests {
    use super::{LeftBehind, Residue, survey};

    /// A state directory of this test's own, with the stems it is handed.
    fn a_state_dir(name: &str, stems: &[(&str, &str, Option<&str>)]) -> std::path::PathBuf {
        // ⛔⛔⛔⛔⛔ SHORT ON PURPOSE, AND THE NEIGHBOUR'S TECHNIQUE — register item 950 ⑴. One case
        // below binds a unix socket under this directory, and `sun_path` is 104 bytes on macOS
        // against 108 on Linux. `sprag-leftovers-{name}-{pid}-ThreadId(N)` plus `/run/<stem>.sock`
        // is 59 bytes of tail, and the macOS runner's scratch root is 48 — **measured 107, four
        // over**, which is why that case failed there on every push while being invisible under
        // Linux's four-byte `/tmp`.
        //
        // ⚠⚠ A per-CALL counter rather than the thread id, which is `cli.rs`'s
        // `default_named_runtime_dir` repair for the same ceiling: the pid keeps two test BINARIES
        // apart and the counter keeps two threads of one binary apart, in three characters instead
        // of thirteen. `sprag_scratch::socket_fits` is what holds this, asserted at the bind.
        static NEXT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let dir = sprag_scratch::scratch_for(&format!("sprag-lo-{name}"), &format!("{n}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("a state directory of this test's own");
        for (stem, runs, snapshot) in stems {
            std::fs::write(dir.join(format!("{stem}.runs.json")), runs).expect("a run log");
            if let Some(snapshot) = snapshot {
                std::fs::write(dir.join(format!("{stem}.snapshot.json")), snapshot)
                    .expect("a snapshot");
            }
        }
        dir
    }

    /// A run log holding `runs` runs, in the shape the product decodes.
    ///
    /// ⚠ `finished` is spelled because the record requires it — a row short of it decodes as
    /// `Unreadable`, which is a KEEP, so a lazy fixture would make the arms below pass for the
    /// wrong reason. That is not hypothetical: the first draft of this fixture did exactly that.
    fn a_log(runs: usize) -> String {
        let rows: Vec<serde_json::Value> = (0..runs)
            .map(|id| {
                serde_json::json!({
                    "id": id, "label": "ai_loop pane=1", "iterations": 1, "finished": true,
                })
            })
            .collect();
        serde_json::json!({"version": crate::runs::RUN_LOG_VERSION, "runs": rows}).to_string()
    }

    /// A snapshot holding `panes` panes, in the shape a daemon really writes.
    ///
    /// ⚠⚠ **THE PANES ARE IN THE SECOND WINDOW, AND THAT IS COPIED FROM THE LIVE FILE.**
    /// `sprag-tui-pty-910212-22.snapshot.json` at 2026-09-06T09:27:54Z has two windows, the first
    /// empty and the second holding the pane — so a count that stopped at the first window would
    /// report this residue as spent and delete it. A one-window fixture cannot catch that.
    ///
    /// ⚠ Every field the record requires is spelled, `sprag-samples`' rule: the decode is the
    /// product's own, so a fixture short of a field reads as UNREADABLE rather than as the shape
    /// it was meant to be — which is how the first draft of this test failed.
    fn a_snapshot(panes: usize) -> String {
        let panes: Vec<serde_json::Value> = (0..panes)
            .map(|id| {
                serde_json::json!({
                    "id": id, "cwd": "/home/coin", "start_dir": "/home/coin",
                    "command_label": "/bin/bash", "argv": ["/bin/bash"],
                    "cols": 80, "rows": 23,
                })
            })
            .collect();
        // ⚠ Every type here is copied from the live file rather than guessed: `current_window` and
        // the window names are STRINGS, `default_size` is a pair and not an object, and a window
        // carries a layout tree. A guess reads as `Unreadable`, which is a keep — so a fixture that
        // drifted would make this gate pass for the wrong reason.
        let window = |name: &str, panes: Vec<serde_json::Value>| {
            serde_json::json!({
                "name": name, "floating": [], "panes": panes,
                "layout": {"nodes": [{"leaf": 0}], "root": 0},
            })
        };
        serde_json::json!({
            "version": 2,
            "sessions": [{
                "name": "0", "current_window": "1",
                "windows": [window("0", Vec::new()), window("1", panes)],
            }],
            "next_id": 1,
            "default_size": [80, 24],
        })
        .to_string()
    }

    /// ⛔⛔⛔⛔⛔ **A RESIDUE THAT STILL HOLDS SOMETHING IS NEVER REMOVABLE, AND THE RUN LOG IS NOT
    /// THE ONLY THING IT CAN HOLD** — register item 905, and the arm its own done-when would have
    /// deleted.
    ///
    /// # ⛔⛔⛔⛔⛔ Item 905 says *a log holding no runs disappears*, and one live row refutes it
    ///
    /// Measured over the loop's state directory at 2026-09-06T09:24:08Z, the pane counts across 69
    /// snapshots were `{0: 67, 1: 1, 7: 1}` — and the stem holding **1 pane logged 0 runs**. Its
    /// run log is empty, so the rule as written removes it, along with the snapshot a successor
    /// daemon on that socket restores from and the `.history/0.hist` holding that pane's
    /// scrollback.
    ///
    /// ⚠⚠ The fixture below is that row, beside the two it must be told apart from.
    #[test]
    fn a_residue_that_still_holds_something_is_never_removable() {
        let empty = a_snapshot(0);
        let dir = a_state_dir(
            "holds",
            &[
                ("gone-and-empty", &a_log(0), Some(&empty)),
                ("gone-with-runs", &a_log(3), Some(&empty)),
                ("gone-with-panes", &a_log(0), Some(&a_snapshot(1))),
                ("gone-and-snapshotless", &a_log(0), None),
                ("unreadable", "{ this is not a run log", Some(&empty)),
            ],
        );
        // ⚠ A runtime directory with no socket in it: every stem here is meant to read as *nothing
        // is listening*, so the daemon question is held constant and the contents decide.
        let nowhere = dir.join("no-sockets");
        std::fs::create_dir_all(&nowhere).expect("an empty runtime directory");

        let found: Vec<Residue> = survey(&dir, &nowhere);
        let of = |stem: &str| -> LeftBehind {
            found
                .iter()
                .find(|residue| residue.stem == stem)
                .unwrap_or_else(|| panic!("{stem} must be surveyed: {found:?}"))
                .what
                .clone()
        };

        assert_eq!(
            of("gone-with-panes"),
            LeftBehind::HoldsPanes(1),
            "⛔⛔⛔⛔⛔ REGISTER ITEM 905: a stem whose run log is EMPTY and whose snapshot holds a \
             pane must not be removable. This is a real row — `sprag-tui-pty-910212-22` at \
             2026-09-06T09:24:08Z, 0 runs and 1 pane — and the item's own done-when, which speaks \
             only of the run log, deletes it together with the scrollback beside it.",
        );
        assert_eq!(of("gone-with-runs"), LeftBehind::HoldsRuns(3));
        assert!(
            matches!(of("unreadable"), LeftBehind::Unreadable(_)),
            "⛔⛔⛔ RULE 6: a file this build cannot read is stated, never passed. *I could not \
             read it* must not become *there was nothing in it* — which is the one mistake here \
             that cannot be undone. Got: {:?}",
            of("unreadable"),
        );
        // 🎯 AND THE ONLY ARM THAT MAY GO, with the snapshotless stem beside it: no snapshot is
        // *no panes*, not *unreadable*, or the common case would be permanently unsweepable.
        assert_eq!(of("gone-and-empty"), LeftBehind::Spent);
        assert_eq!(of("gone-and-snapshotless"), LeftBehind::Spent);

        // ⛔⛔⛔ AND THE VERDICT DECIDES REMOVAL, so a caller cannot spell a second rule.
        let removable: Vec<&str> = found
            .iter()
            .filter(|residue| residue.what.may_remove())
            .map(|residue| residue.stem.as_str())
            .collect();
        assert_eq!(
            removable,
            ["gone-and-empty", "gone-and-snapshotless"],
            "⛔⛔⛔⛔⛔ REGISTER ITEM 905: exactly the spent stems may go. Anything else here is a \
             sweep that removes a record or a session tree. Survey: {found:?}",
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// ⛔⛔⛔⛔⛔ **A LIVE DAEMON'S RESIDUE IS NEVER REMOVABLE, WHATEVER ITS FILES SAY** — register
    /// item 905 ⑵, the mutation the item names.
    ///
    /// # ⛔⛔⛔⛔⛔ Why the socket is asked BEFORE the contents, and not as a tie-break
    ///
    /// A daemon that started a minute ago has an empty run log and an empty snapshot, and is
    /// **indistinguishable by content** from the 67 dead stems beside it. Measured
    /// 2026-09-06T09:23:37Z: of 69 stems, exactly one had a socket, and it was the same stem as the
    /// only one holding runs — so a predicate resting on contents alone is right about every row in
    /// that directory today, and wrong the first time a fresh daemon is swept.
    ///
    /// ⇒ The fixture is that trap exactly: a stem whose files are as empty as a spent one's, with
    /// something answering on its socket.
    #[test]
    fn a_live_daemons_residue_is_never_removable_however_empty_its_files_are() {
        let dir = a_state_dir("live", &[("listening", &a_log(0), Some(&a_snapshot(0)))]);
        let runtime = dir.join("run");
        std::fs::create_dir_all(&runtime).expect("a runtime directory of this test's own");

        // ⚠ A listener that accepts and never speaks: `survey::ask` calls that `Refused`, which is
        // the arm that matters most here — *not a daemon I recognise* must not read as *nobody is
        // there*. A real daemon would be `Serving`, and both are keeps.
        // ⛔⛔⛔⛔⛔ **THROUGH THE CHECKING DOOR, ON EVERY PLATFORM** — register item 950 ⑴, and
        // item 955 is why it is this door and not a bare assertion. `bind` refuses a path over
        // `sun_path` with *"path must be shorter than SUN_LEN"*, and that limit is 104 on macOS
        // against 108 on Linux — so this test passed here and failed on the macOS runner on every
        // push. The check measures the part below the scratch root against the LONGEST root this
        // project must tolerate (48 bytes, measured on that runner), which is what makes the answer
        // the same on this machine as on that one.
        //
        // ⚠ It was written as `assert!(socket_fits(…))` when item 950 paid it, and item 955's gate
        // refused that: twenty other sites needed the same check, so the shape that spreads has to
        // be the one that HANDS THE PATH BACK. One spelling, twenty-one sites.
        let socket = sprag_scratch::may_bind(&runtime.join("listening.sock"));
        let listener =
            std::os::unix::net::UnixListener::bind(&socket).expect("a socket of this test's own");

        let found = survey(&dir, &runtime);
        let residue = found.first().expect("the stem is surveyed");
        assert!(
            !residue.what.may_remove(),
            "⛔⛔⛔⛔⛔ REGISTER ITEM 905 ⑵: THIS SWEEP WOULD HAVE DELETED A LIVE DAEMON'S FILES. \
             Its run log and snapshot are as empty as a spent stem's, because a daemon that has \
             just started HAS an empty run log — the contents cannot tell them apart, and only the \
             socket can. Got {:?}",
            residue.what,
        );
        assert!(
            matches!(residue.what, LeftBehind::Listening(_)),
            "⚠⚠ AND IT IS KEPT FOR THE RIGHT REASON — something is on that socket. A keep reached \
             through the contents would go the other way the moment the files were empty. Got {:?}",
            residue.what,
        );
        drop(listener);
        let _ = std::fs::remove_file(&socket);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// ⚠⚠ **EVERY ARM SAYS SOMETHING DIFFERENT AND ONLY ONE OF THEM MAY GO** — this workspace's
    /// rule for a vocabulary, over the arms rather than over a directory.
    #[test]
    fn every_verdict_says_its_own_thing_and_one_of_them_may_go() {
        let arms = [
            LeftBehind::Serving,
            LeftBehind::Listening("older wire".to_owned()),
            LeftBehind::HoldsRuns(3),
            LeftBehind::HoldsPanes(1),
            LeftBehind::Spent,
            LeftBehind::Unreadable("truncated".to_owned()),
        ];
        let mut words: Vec<&str> = arms.iter().map(LeftBehind::word).collect();
        words.sort_unstable();
        words.dedup();
        assert_eq!(
            words.len(),
            arms.len(),
            "⚠ two arms sharing a word is a reader unable to branch on it: {words:?}",
        );
        assert_eq!(
            arms.iter().filter(|arm| arm.may_remove()).count(),
            1,
            "⛔⛔⛔⛔⛔ REGISTER ITEM 905: EXACTLY ONE arm may be removed. A second one is a sweep \
             that throws away a record, a session tree, or a file this build merely could not \
             read — and the last of those is the one that reads as success.",
        );
        for arm in &arms {
            assert!(
                arm.describe().len() > 30,
                "⚠ every arm explains itself; a person who is told *no* has to know which no it \
                 is: {arm:?}",
            );
        }
    }
}
