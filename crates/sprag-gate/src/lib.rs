//! The gates a test cannot be.
//!
//! # Why a crate outside the suite
//!
//! Some claims are about what the SUITE ITSELF did, and a test cannot make one. R341 measured
//! `cargo test` creating `~/.config/sprag/config.toml` on the developer's box — a test ran
//! `sprag set-option` through a helper that passed no environment, so the child resolved the
//! AMBIENT config home, and a SIBLING test then read that file and was green here and red on every
//! machine without it. The fix for any ONE call site is a seam. Nothing makes the NEXT call site
//! take one, and no test can be the guard: `XDG_CONFIG_HOME` is process-global, so a test can
//! neither observe nor isolate what the tests running beside it do to it.
//!
//! So the guard has to be a separate process, run after the suite. This crate is where those live.
//!
//! # ⚠ And the first one shipped broken, which is why the logic is here and not in the yaml
//!
//! R342 wrote that guard as three lines of shell in `ci.yml`: `find "$RUNNER_TEMP/ambient"
//! -mindepth 1`. `$RUNNER_TEMP/ambient` is the PARENT of the three homes, and the test step's own
//! `mkdir -p` creates them, so the find always returned three directories and the step **failed
//! unconditionally**. It never once passed. Both Linux runs that carried it were red for that
//! reason, and the round that added it recorded a green measured on the commits before it.
//!
//! The defect was not the depth argument. It was that **the guard looked somewhere other than
//! where the suite wrote** — so [`ambient_homes`] derives its paths from the same three environment
//! variables the suite ran under, and a variable that is unset or does not name a directory is an
//! ERROR rather than an empty walk. A probe pointed at nothing must never read as clean.

/// What a stand-in program is, and the rule that no suite writes one — register item 467.
///
/// It lives here rather than in any one suite for the reason the rest of this crate does: the claim
/// is about what a SUITE DID (it manufactured its own executable and then raced the harness to run
/// it), so no single test can be its own guard. `unix` only, because a double is a program and a
/// link, and this workspace's suites run on Linux and macOS.
///
/// ⚠⚠ **It is also where a suite asks WHERE THIS MACHINE KEEPS a program** — [`doubles::system`],
/// register item 472. Those two platforms disagree about `/bin`: on Linux it is a symlink to
/// `/usr/bin` and so holds everything, while macOS's is a real directory of about thirty programs
/// with neither `true` nor `false` among them. A spelled `/bin/true` is therefore green on every
/// Linux sweep and `NotFound` on the macOS job, which is how items 467 and 471 each shipped one.
#[cfg(unix)]
pub mod doubles;

/// Handing a child its standard input, and surviving one that refuses first — register item 471.
///
/// Here for the same reason as [`doubles`]: three suites in this workspace fed a child's stdin and
/// treated `EPIPE` as fatal, so the claim is about what SUITES do rather than about any one of
/// them, and a rule kept in one of them is a rule the next one does not get.
pub mod feeding;

/// Where the debt-repayment loop's DECISIONS live, so a regression cannot be silent — item 470.
///
/// Here rather than in `sprag-plugin` for [`sources`]'s reason: the claim is about the TEXT of this
/// workspace's Rust measured against a document, the walk is the same walk, and a second copy of it
/// is where two copies of a rule drift apart.
pub mod loop_shape;

/// Which of the loop template's numbers a KIND is invited to author, and whether anything can carry
/// one — item 494.
///
/// Beside [`loop_shape`] and for its reason: both read the loop's document against this
/// workspace's Rust, and item 492 paid one instance of a defect whose CLASS is what this closes.
pub mod authored;

/// Whether the loop's economic edge is priced in the POPULATION it will run in — item 493.
///
/// Beside [`authored`] and for the same reason: the claim spans the template's prose and the Rust
/// that drives it, so neither side alone can hold it. Item 494 pinned which numbers a KIND may
/// author; this pins that a MEASURED one still describes the sessions it was measured on.
pub mod economics;

/// Whether an event the loop's document reads `_event.data` off is ever raised WITHOUT it —
/// item 507.
///
/// Beside [`economics`] and [`authored`] for their reason, and it is the third face of the same
/// claim: those two pin what the document is TOLD, this pins that it is told it AT ALL. Fifteen
/// fixture sites raised `turn.done` and `judge` with no `_event.data`, so `judging`'s entry block
/// was abandoned on every one of them, and W3C SCXML 3.12.2 dropped the error — green for months.
pub mod payload;

/// Whether every party that WRITES an input event names its fields from one place — item 559.
///
/// Beside [`payload`] and for its reason: the claim is that a RENAME cannot leave a writer behind,
/// which is a fact about the workspace's text rather than about any one crate, and a rule kept in
/// one crate is a rule the next writer does not get. A running daemon cannot answer it — it can
/// only say that today's spelling works, which is exactly what let the split ship green.
pub mod vocabulary;

/// Every Rust source this workspace carries, for the gates that judge the TEXT of it.
///
/// Shared by the two workspace-wide ratchets (items 467 and 471) rather than copied into each: the
/// walk's fiddly parts are where two copies of a rule drift apart.
pub mod sources;

/// Which PACKAGE owns a binary, so a refusal can name the command that ENDS it — item 455.
///
/// Here rather than inside [`Unbuilt`] because the question is about the workspace, not about one
/// refusal: the same map answers for every binary a guard can ever be handed, including the ones
/// no package has grown yet.
pub mod owners;

/// Whether a SWEEP swept — register item 585, and the clearest case of this crate's own subject.
///
/// A suite cannot report that it did not run, so no test inside a sweep can notice the nineteen
/// crates the sweep never reached. The module derives the expectation from the workspace rather
/// than holding a number.
pub mod sweep;

/// How much of a pseudoterminal namespace ONE process asked for — register item 817.
///
/// Beside [`sweep`] and for its reason, on a different quantity: the number is a process total, so
/// inside the process it is whatever the threads beside the assertion have reached and it is not
/// final until that process has exited. What made it debt is that it used to be readable only out
/// of a REFUSAL — the one moment it is too late to act on, and on macOS the only one, since there
/// is no `strace` there to take it any other way.
pub mod pty_demand;

/// Whether every pseudoterminal comes from the one door that explains a refusal — register item
/// 776, arm (d).
///
/// A bare `ENXIO` from a second call site would carry none of the pool's size, the host's in-use
/// count or this process's own share, and the reader would complete it from memory — which is how
/// *the runner's pty pool was exhausted* got written down with nothing behind it.
pub mod refusals;

/// What the north star is COUNTING — register item 823.
///
/// Beside the others for their reason, on the one quantity that is about the ledger rather than
/// about the code: *"zero unpaid ai-loop items"* had two predicates on 2026-09-02 and they returned
/// 13 and 45. No test inside the workspace can settle that, because the subject is a document the
/// workspace does not contain — so the population becomes a MARK each item states, and this reads
/// it.
pub mod north_star;

use std::ffi::OsString;
use std::fmt;
use std::path::{Path, PathBuf};

/// The three environment variables an XDG-respecting process writes under, in the order a report
/// reads best.
///
/// `XDG_DATA_HOME` is here even though no sprag reader resolves one today: the claim this guard
/// makes is *the suite wrote nothing outside what it was given*, and a variable nobody reads yet is
/// exactly where the next call site will write without anybody noticing.
pub const AMBIENT_HOMES: [&str; 3] = ["XDG_CONFIG_HOME", "XDG_DATA_HOME", "XDG_STATE_HOME"];

/// A home this guard was asked to watch and could not.
///
/// Its own type because the alternative is the defect this crate exists for: a mis-pointed probe
/// that walks nothing, finds nothing, and reports the suite clean.
#[derive(Debug, PartialEq, Eq)]
pub enum Unwatchable {
    /// The variable is not set at all — nobody told this process where to look.
    Unset(&'static str),
    /// It is set, and what it names is not a directory that can be read.
    Unreadable {
        /// The variable that named it.
        var: &'static str,
        /// What it named.
        path: PathBuf,
        /// Why the walk could not start.
        why: String,
    },
}

impl fmt::Display for Unwatchable {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unset(var) => write!(
                f,
                "{var} is not set, so this guard has no idea where the suite was pointed. \
                 Run the suite and this guard under the SAME three variables."
            ),
            Self::Unreadable { var, path, why } => write!(
                f,
                "{var} names {} and it cannot be walked ({why}). A guard that cannot read the \
                 directory it is judging must say so rather than report it empty.",
                path.display()
            ),
        }
    }
}

/// Every path under `home`, at any depth — empty exactly when nothing was written there.
///
/// RECURSIVE, and that is the substance rather than a detail: the write this guard exists to catch
/// is `<config home>/sprag/config.toml`, which is two levels down. A walk that only listed the
/// entries of `home` itself would see `sprag/` and could not tell a directory somebody made from a
/// file somebody wrote — and one that started a level too high, as the shell version did, sees the
/// homes themselves and can never be quiet at all.
///
/// # Errors
///
/// If `home` cannot be read as a directory. **Never treated as "nothing was written"**: a probe
/// that names the wrong thing answers about the wrong thing, and this project has now spent four
/// rounds on that one shape.
pub fn writes_under(home: &Path) -> std::io::Result<Vec<PathBuf>> {
    let mut found = Vec::new();
    let mut walking = vec![home.to_path_buf()];
    while let Some(dir) = walking.pop() {
        for entry in std::fs::read_dir(&dir)? {
            let path = entry?.path();
            if path.is_dir() {
                walking.push(path.clone());
            }
            found.push(path);
        }
    }
    found.sort();
    Ok(found)
}

/// The three homes named by [`AMBIENT_HOMES`] in this process's environment, or the first one that
/// cannot be watched.
///
/// # Errors
///
/// If any of the three is unset, or names something that cannot be walked.
pub fn ambient_homes() -> Result<Vec<(&'static str, PathBuf)>, Unwatchable> {
    homes_from(std::env::var_os)
}

/// [`ambient_homes`] against a stated environment rather than the process's — the seam its own
/// tests need, since the real one is process-global and this crate's tests run as threads of one
/// binary (the very property that makes a test unable to be the guard).
fn homes_from(
    lookup: impl Fn(&'static str) -> Option<OsString>,
) -> Result<Vec<(&'static str, PathBuf)>, Unwatchable> {
    let mut homes = Vec::with_capacity(AMBIENT_HOMES.len());
    for var in AMBIENT_HOMES {
        let value = lookup(var).ok_or(Unwatchable::Unset(var))?;
        let path = PathBuf::from(value);
        // Read once HERE rather than left to the walk, so "you pointed me at a file" and "you
        // pointed me at nothing" are one answer with one shape.
        std::fs::read_dir(&path).map_err(|why| Unwatchable::Unreadable {
            var,
            path: path.clone(),
            why: why.to_string(),
        })?;
        homes.push((var, path));
    }
    Ok(homes)
}

/// Why a sibling binary a suite drives cannot be trusted.
///
/// Its own type for [`Unwatchable`]'s reason exactly: every arm here is a state in which the
/// question *"is this the code I edited?"* has NO answer, and this crate's whole doctrine is that a
/// probe which cannot tell must never read as clean.
///
/// # ⚠⚠⚠ Every arm carries a remedy, and the remedy is DERIVED — register item 455
///
/// A refusal is only half a gate; the other half is the command that ends it. All three arms once
/// spelled that command as `cargo build -p sprag-host --bins`, which was true of the two binaries
/// this guard covered on the day it was written and false of `sprag-mcp`, **its own package**. The
/// advice was followed exactly on a build machine and the same refusal came back with the same
/// words. [`owners::build_command`] now reads the package off this workspace's manifests, so the
/// next binary to be added earns a sentence that works rather than the same wrong one.
#[derive(Debug, PartialEq, Eq)]
pub enum Unbuilt {
    /// Nothing is there at all — cargo never built it for this package.
    Missing(PathBuf),
    /// It is there, and cargo's record of WHAT IT WAS BUILT FROM is not beside it, so its freshness
    /// is unknowable.
    Unrecorded {
        /// The binary.
        bin: PathBuf,
        /// The depfile that is missing or unreadable.
        depfile: PathBuf,
        /// Why it could not be read.
        why: String,
    },
    /// It is there and it was built from source that has been EDITED SINCE — the case that lies.
    Stale {
        /// The binary.
        bin: PathBuf,
        /// The inputs newer than it, in the order cargo recorded them.
        edited: Vec<PathBuf>,
    },
}

impl fmt::Display for Unbuilt {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Missing(bin) => write!(
                f,
                "{} is not built. This suite drives a binary that belongs to ANOTHER package, which \
                 the `-p` that built the suite does not reach — run `{}` first, or `cargo test \
                 --workspace`.",
                bin.display(),
                owners::build_command(bin),
            ),
            Self::Unrecorded { bin, depfile, why } => write!(
                f,
                "{} is there and {} is not readable ({why}), so whether it was built from the \
                 source in this tree cannot be answered. A run that cannot tell must not pass: \
                 rebuild it with `{}`.",
                bin.display(),
                depfile.display(),
                owners::build_command(bin),
            ),
            Self::Stale { bin, edited } => {
                write!(
                    f,
                    "{} IS STALE — {} of the sources cargo built it from have been edited since, \
                     so this run would be about code that is not in this tree. Run `{}` first.\n  \
                     newer than the binary:",
                    bin.display(),
                    edited.len(),
                    owners::build_command(bin),
                )?;
                for path in edited.iter().take(STALE_REPORT_CAP) {
                    write!(f, "\n    {}", path.display())?;
                }
                if let Some(rest) = edited
                    .len()
                    .checked_sub(STALE_REPORT_CAP)
                    .filter(|n| *n > 0)
                {
                    write!(f, "\n    ...and {rest} more")?;
                }
                Ok(())
            }
        }
    }
}

/// How many newer inputs a [`Unbuilt::Stale`] report NAMES before summarising the rest.
///
/// A whole dependency closure can be newer after a `touch -r`, and a panic message that pastes six
/// hundred paths buries the sentence that says what to do.
const STALE_REPORT_CAP: usize = 5;

/// The sources `bin` was built from that have been EDITED SINCE it was built — empty for a binary
/// that is current.
///
/// # ⚠⚠⚠ Why this exists, and why it asks CARGO rather than guessing
///
/// A test that spawns a binary from another package is asking a question about code it did not
/// compile. `cargo test -p sprag-mcp` builds the `sprag-host` LIB and never the `sprag-term` BIN,
/// so a change to daemon-side code is invisible to that suite and **a revert-proof measured through
/// it passes**. The absence check both call sites already had cannot see it: the binary EXISTS, it
/// is simply older than the edit, and that is the case that lies.
///
/// Measured three times: R241 (a forced `arbitrate` left three window-size tests green; rebuilt,
/// two went red), R284 (a pixel-smoke "regression" that was a day-old `sprag-tui`), and R367 — where
/// a mutation dropped a whole wire key and the end-to-end gate stayed GREEN until the bins were
/// rebuilt. Every one of those was a rule somebody was supposed to remember.
///
/// **Cargo already writes the answer.** Beside every binary it links is a depfile — `sprag-term.d`
/// — holding the target and every source that went into it, across the WHOLE dependency closure
/// (eight crates here, generated `OUT_DIR` sources included). So this hard-codes no crate list, runs
/// no nested cargo, and cannot drift when the graph changes: the question it asks is cargo's own,
/// off cargo's own record.
///
/// ⚠ It is also cargo's own REBUILD condition, which is what makes the remedy always work: an input
/// newer than the output is exactly what makes `cargo build` relink, so a binary this reports on is
/// one cargo will refresh.
///
/// # Errors
///
/// [`Unbuilt::Missing`] when nothing is there, [`Unbuilt::Unrecorded`] when the depfile is not —
/// never `Ok(vec![])`, which is this crate's standing rule about probes that cannot see.
pub fn edited_since_built(bin: &Path) -> Result<Vec<PathBuf>, Unbuilt> {
    let built = std::fs::metadata(bin)
        .and_then(|meta| meta.modified())
        .map_err(|_| Unbuilt::Missing(bin.to_path_buf()))?;
    let depfile = bin.with_extension("d");
    let record = std::fs::read_to_string(&depfile).map_err(|why| Unbuilt::Unrecorded {
        bin: bin.to_path_buf(),
        depfile: depfile.clone(),
        why: why.to_string(),
    })?;
    // `<target>: <input> <input> ...`, one rule per line. Only the inputs are of interest, and the
    // target is re-derived rather than trusted — a depfile naming another binary is still a list of
    // this one's inputs as far as this check goes, and refusing it would be stricter than the
    // question being asked.
    let mut edited = Vec::new();
    let mut inputs_seen = Vec::new();
    for line in record.lines() {
        let Some((_, inputs)) = line.split_once(": ") else {
            continue;
        };
        for input in inputs.split_whitespace().map(Path::new) {
            inputs_seen.push(input.to_path_buf());
            // A source that no longer EXISTS counts as edited: its removal is a change, and a
            // binary built from a file that is gone is exactly as untrustworthy as one built from a
            // file that moved on.
            let newer = std::fs::metadata(input)
                .and_then(|meta| meta.modified())
                .map_or(true, |touched| touched > built);
            if newer {
                edited.push(input.to_path_buf());
            }
        }
    }
    // ⚠⚠⚠⚠ **AND NOW THE SAME EVIDENCE CARGO USES, BECAUSE mtime ALONE MADE THIS GATE PRINT A
    // REMEDY NOBODY COULD PERFORM** — register item 221, measured end to end.
    //
    // Cargo relinks on a FINGERPRINT and this decided on a TIMESTAMP, so an edit that regenerates
    // codegen with byte-identical output moved every mtime while cargo — correctly — refused to
    // rebuild. This gate then refused 65 of 67 targets with *"run `cargo build -p sprag-host
    // --bins` first"*, which reported `Fresh` in a second and changed nothing; the only escape was
    // deleting the binary. Every scxml edit produces that, which is to say the loop met it whenever
    // it improved its own document, and what it saw was a red that was not its own.
    //
    // So a newer mtime is now a QUESTION rather than a verdict: the contents of the recorded inputs
    // are fingerprinted and compared with what they were when this binary was last seen fresh. A
    // content-identical regeneration is green **by construction**, and a real edit is still red
    // because its bytes differ.
    //
    // ⚠⚠ **A MISSING RECORD STAYS RED**, which is this crate's standing doctrine: with nothing to
    // compare against, *"is this the code I edited?"* has no answer, and a probe that cannot tell
    // must never read as clean. The first green check after a build lays the record down.
    let ledger = fingerprint_path(bin);
    if edited.is_empty() {
        // Fresh by the cheap question — record what fresh looked like, best-effort. A record that
        // cannot be written costs the NEXT content-identical regeneration a red, which is the safe
        // direction and never a wrong pass.
        let _ = std::fs::write(&ledger, fingerprint_of(&inputs_seen));
        return Ok(Vec::new());
    }
    if let Ok(was) = std::fs::read_to_string(&ledger)
        && was == fingerprint_of(&inputs_seen)
    {
        return Ok(Vec::new());
    }
    Ok(edited)
}

/// Where the record of *what fresh looked like* sits — beside the binary, in `target/`, so it is
/// removed by the same `cargo clean` that removes what it describes.
fn fingerprint_path(bin: &Path) -> PathBuf {
    let mut name = bin.file_name().unwrap_or_default().to_os_string();
    name.push(".sprag-inputs");
    bin.with_file_name(name)
}

/// A content fingerprint of the recorded inputs — **FNV-1a, hand-rolled, and both halves of that
/// are deliberate.**
///
/// This crate has no dependencies on purpose (*"a gate that stands outside the suite must not be
/// able to fail because the product failed to compile"*), so the hash is written here. FNV rather
/// than [`std::collections::hash_map::DefaultHasher`] because this value is PERSISTED between
/// runs and `DefaultHasher`'s output is explicitly not guaranteed stable across Rust releases — a
/// toolchain bump would silently invalidate every record, which is only a spurious red, but a
/// spurious red is the exact thing item 221 is about.
///
/// ⚠ The PATH is hashed beside the bytes, so a depfile whose input list changed is a different
/// fingerprint even if the files it now names happen to hold the same text.
fn fingerprint_of(inputs: &[PathBuf]) -> String {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    let mut eat = |bytes: &[u8]| {
        for byte in bytes {
            hash ^= u64::from(*byte);
            hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
        }
    };
    for input in inputs {
        eat(input.as_os_str().as_encoded_bytes());
        // ⚠ A source that cannot be READ hashes as its own absence rather than as empty, so a file
        // that vanished and one that was truncated are different fingerprints.
        match std::fs::read(input) {
            Ok(bytes) => eat(&bytes),
            Err(why) => eat(why.kind().to_string().as_bytes()),
        }
    }
    format!("{hash:016x}")
}

/// A binary belonging to ANOTHER package, beside the one cargo built for the test calling this —
/// PANICKING unless it exists and is current.
///
/// `own_exe` is the caller's own `env!("CARGO_BIN_EXE_<name>")`: cargo sets that only for binaries
/// of the package under test, which is the whole reason a sibling has to be derived rather than
/// given.
///
/// # ⚠⚠ Why one function rather than a check at each spawn site
///
/// There were two derivations of this path when it was written — `sprag-tui`'s pty suite and
/// `sprag-mcp`'s stdio suite — with near-identical comments and the same absence-only check, which
/// is the drift shape this tree keeps paying to remove. More to the point, a check each site has to
/// remember is the thing that already failed three times; the point of moving it here is that a
/// site cannot spawn a sibling WITHOUT it.
///
/// # Panics
///
/// If the binary is missing, unrecorded, or stale — [`Unbuilt`] carries the remedy in each case.
/// A panic rather than a `Result` so no call site can decide to carry on: the run that follows
/// would be about the wrong code.
#[must_use]
pub fn sibling_bin(own_exe: &str, name: &str) -> PathBuf {
    let bin = Path::new(own_exe)
        .parent()
        .expect("a built binary has a directory")
        .join(name);
    match edited_since_built(&bin) {
        Ok(edited) if edited.is_empty() => bin,
        Ok(edited) => panic!("{}", Unbuilt::Stale { bin, edited }),
        Err(unbuilt) => panic!("{unbuilt}"),
    }
}

#[cfg(test)]
mod freshness_tests {
    use super::*;
    use std::time::{Duration, SystemTime};

    /// ⚠⚠⚠⚠ **A REGENERATION THAT CHANGED NO BYTE IS FRESH, AND A CHANGED BYTE IS NOT** — register
    /// item 221, which measured this gate refusing **65 of 67 targets** and printing a remedy that
    /// reported `Fresh` in a second and changed nothing.
    ///
    /// # ⚠⚠⚠ Why both halves, in one gate
    ///
    /// A fix that always answers *fresh* passes the first half on its own — and that fix is exactly
    /// what "just stop checking" looks like. The second half is what makes the first mean anything:
    /// one byte of a real source, no rebuild, still red.
    ///
    /// ⚠⚠ The first half is TODAY'S DEFECT reproduced in one call, with no load and no timing: an
    /// edit to `ai_loop.scxml` makes cargo regenerate `out/*.rs` byte-identically, so every mtime
    /// moves while cargo's fingerprint does not — and this gate decided on the mtime.
    #[test]
    fn a_touched_source_is_fresh_when_its_bytes_did_not_change_and_red_when_they_did() {
        let tree = BuiltTree::new("content", &[("a.rs", -60), ("b.rs", -60)]);
        assert_eq!(
            edited_since_built(&tree.bin),
            Ok(Vec::new()),
            "⚠ the control: sources older than the binary are fresh, and this is the pass that \
             lays down the record the two halves below are read against",
        );

        // ── THE REGENERATION ── every input's mtime moves, not one byte changes.
        let touched = SystemTime::now();
        for name in ["a.rs", "b.rs"] {
            set_mtime(&tree.dir.join(name), touched);
        }
        assert_eq!(
            edited_since_built(&tree.bin),
            Ok(Vec::new()),
            "⚠⚠⚠⚠ A BYTE-IDENTICAL REGENERATION READ AS STALE. This is item 221: cargo relinks on a \
             FINGERPRINT and correctly refuses here, so the remedy this gate prints — `cargo build \
             -p sprag-host --bins` — reports `Fresh` and changes nothing, and the only escape is \
             deleting the binary. Every scxml edit produces exactly this.",
        );

        // ── THE REAL EDIT ── one byte, no rebuild.
        std::fs::write(tree.dir.join("a.rs"), b"fn main() {/**/}").expect("edit a source");
        set_mtime(&tree.dir.join("a.rs"), touched);
        let answer = edited_since_built(&tree.bin).expect("the binary and its depfile are there");
        assert!(
            answer.contains(&tree.dir.join("a.rs")),
            "⚠⚠⚠⚠ A CHANGED SOURCE READ AS FRESH, which is the failure the whole check exists to \
             prevent: the run would then be about code that is not in this tree. Got {answer:?}",
        );
        // ⚠⚠ **THE VERDICT IS CONTENT, THE LIST IS STILL mtime — a SUPERSET, and said so rather
        // than left to be discovered.** `b.rs` was touched and not edited, and it is named here
        // beside `a.rs` because the record kept is ONE fingerprint over all the inputs, not one per
        // file. That is honest for the field's own doc (*"the inputs newer than it"*) and it is the
        // residue of the cheap record: a reader is pointed at a set that certainly contains the
        // change rather than at the change itself. Per-file hashes would narrow it, at a record
        // that grows with the depfile.
        assert!(
            answer.contains(&tree.dir.join("b.rs")),
            "the list is the mtime-newer set, so this is the shape to notice if it ever narrows",
        );
    }

    /// ⚠⚠ **AND WITH NO RECORD TO COMPARE AGAINST, A NEWER SOURCE IS STILL RED** — this crate's
    /// standing doctrine, which the content check must not soften: *"a probe which cannot tell must
    /// never read as clean"*. The record is laid down by a green check, so a binary whose inputs
    /// already look edited before anything has ever passed has nothing to be compared with.
    #[test]
    fn a_newer_source_with_no_record_yet_is_refused_rather_than_guessed_at() {
        let tree = BuiltTree::new("norecord", &[("a.rs", 60)]);
        assert_eq!(
            edited_since_built(&tree.bin),
            Ok(vec![tree.dir.join("a.rs")]),
            "a gate with nothing to compare against must refuse, not assume",
        );
    }

    /// A fake `target/debug` holding one binary, its depfile, and the sources it names — the shape
    /// cargo leaves behind, built by hand so the MTIMES can be stated rather than raced.
    ///
    /// The binary is stamped OLDER than now and each source is placed relative to it, because the
    /// whole subject here is an ordering that a real build produces over minutes and a test has to
    /// produce in one call.
    struct BuiltTree {
        dir: PathBuf,
        bin: PathBuf,
    }

    impl BuiltTree {
        /// `sources` is `(name, seconds relative to the binary's own mtime)` — negative is a source
        /// the binary was built AFTER (the healthy case), positive is one edited since.
        fn new(tag: &str, sources: &[(&str, i64)]) -> Self {
            let dir =
                std::env::temp_dir().join(format!("sprag-gate-fresh-{}-{tag}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).expect("the fake target dir");
            let bin = dir.join("sprag-term");
            std::fs::write(&bin, b"ELF").expect("the fake binary");
            let built = SystemTime::now() - Duration::from_secs(3600);
            set_mtime(&bin, built);

            let mut inputs = Vec::new();
            for (name, offset) in sources {
                let path = dir.join(name);
                std::fs::write(&path, b"fn main() {}").expect("a fake source");
                set_mtime(&path, shifted(built, *offset));
                inputs.push(path.display().to_string());
            }
            std::fs::write(
                bin.with_extension("d"),
                format!("{}: {}\n", bin.display(), inputs.join(" ")),
            )
            .expect("the fake depfile");
            Self { dir, bin }
        }
    }

    impl Drop for BuiltTree {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    fn shifted(from: SystemTime, seconds: i64) -> SystemTime {
        let by = Duration::from_secs(seconds.unsigned_abs());
        if seconds < 0 { from - by } else { from + by }
    }

    /// Stamp a file's mtime, so an ordering a real build produces over minutes can be STATED here
    /// rather than raced. The alternative — sleeping between writes — would make every assertion
    /// below a bet on the filesystem's timestamp granularity.
    ///
    /// # ⚠⚠⚠ It shelled out to `touch -d @<epoch>`, and the comment beside it said that was POSIX
    ///
    /// It is not. `-d @<seconds>` is a GNU coreutils extension: BSD `touch` — which is what macOS
    /// ships — reads `-d` as an ISO-8601 timestamp and refuses an epoch, so all four of these gates
    /// failed on the macOS runner in the commit that added them, and the whole `sprag-gate` target
    /// with them. **A comment that states a premise is a claim to test**, and this one was written
    /// on the box where it happened to be true.
    ///
    /// [`File::set_times`] is std's own answer and has been since 1.75, which is well under this
    /// workspace's `rust-version`. It takes no dependency, runs no process, and has no dialect —
    /// so there is no second platform for it to be wrong on.
    fn set_mtime(path: &Path, when: SystemTime) {
        std::fs::File::options()
            .write(true)
            .open(path)
            .expect("the file to stamp")
            .set_times(std::fs::FileTimes::new().set_modified(when))
            .unwrap_or_else(|why| panic!("stamping {} failed: {why}", path.display()));
    }

    /// **A BINARY NEWER THAN EVERY SOURCE CARGO BUILT IT FROM IS CURRENT** — the healthy reading,
    /// asserted first so the reds below are not a probe that always fires.
    #[test]
    fn a_binary_built_after_its_sources_is_reported_current() {
        let tree = BuiltTree::new("current", &[("a.rs", -60), ("b.rs", -30)]);
        assert_eq!(
            edited_since_built(&tree.bin),
            Ok(Vec::new()),
            "nothing has moved since the link, so there is nothing to report",
        );
    }

    /// **THE CASE THAT LIES**: the binary is THERE, so every absence check this replaced passes,
    /// and one of its sources has been edited since.
    ///
    /// This is R367's mutation exactly — a source changed, the test binary rebuilt, the daemon not.
    /// REVERT-PROOF: compare `>=` instead of `>` and the current case above reddens; drop the
    /// missing-input arm and the sibling test below goes green on a deleted source.
    #[test]
    fn a_source_edited_since_the_link_makes_the_binary_stale() {
        let tree = BuiltTree::new("stale", &[("fresh.rs", -60), ("edited.rs", 60)]);
        let edited =
            edited_since_built(&tree.bin).expect("the binary and its record are both there");
        assert_eq!(
            edited,
            vec![tree.dir.join("edited.rs")],
            "only the input newer than the binary is named, and it IS named",
        );
        let said = Unbuilt::Stale {
            bin: tree.bin.clone(),
            edited,
        }
        .to_string();
        assert!(
            said.contains("IS STALE") && said.contains("cargo build -p sprag-host --bins"),
            "the report has to carry the remedy, or it is a puzzle rather than a gate: {said}",
        );
    }

    /// A source cargo recorded and that is now GONE counts as edited: a binary built from a file
    /// that no longer exists is exactly as untrustworthy as one built from a file that moved on.
    #[test]
    fn a_source_that_no_longer_exists_makes_the_binary_stale() {
        let tree = BuiltTree::new("removed", &[("gone.rs", -60)]);
        std::fs::remove_file(tree.dir.join("gone.rs")).expect("remove the recorded source");
        assert_eq!(
            edited_since_built(&tree.bin),
            Ok(vec![tree.dir.join("gone.rs")]),
            "a recorded input that cannot be found must not read as unchanged",
        );
    }

    /// **A BINARY WITH NO RECORD IS A REFUSAL, NOT A PASS** — this crate's whole doctrine, applied
    /// to its newest probe. Without this arm a missing depfile would report `Ok(vec![])`, which is
    /// the "clean" that the shell guard R342 shipped could not stop saying.
    #[test]
    fn a_binary_whose_record_is_missing_is_refused_rather_than_believed() {
        let tree = BuiltTree::new("unrecorded", &[("a.rs", -60)]);
        std::fs::remove_file(tree.bin.with_extension("d")).expect("remove the record");
        let why = edited_since_built(&tree.bin).expect_err("no record is no answer");
        assert!(
            matches!(why, Unbuilt::Unrecorded { .. }),
            "an unreadable record is its own arm: {why:?}",
        );
        assert!(
            why.to_string().contains("cannot be answered"),
            "and it says so rather than implying cleanliness: {why}",
        );
    }

    /// ...and a binary that was never built at all is the arm both call sites already had, kept.
    #[test]
    fn a_binary_that_was_never_built_is_named_as_missing() {
        let dir = std::env::temp_dir().join(format!("sprag-gate-absent-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("an empty dir");
        let bin = dir.join("sprag-term");
        assert_eq!(
            edited_since_built(&bin),
            Err(Unbuilt::Missing(bin.clone())),
            "nothing there is a different failure from something stale, and needs a different fix",
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// **THE GATE'S OWN SUBJECT, AGAINST THE REAL BUILD**: the depfile cargo writes beside the
    /// daemon this workspace actually ships covers MORE THAN ONE CRATE.
    ///
    /// The whole design rests on that — if cargo recorded only `sprag-host`'s own sources, a change
    /// in `sprag-vt` would still slip through and this check would be a comfortable lie. Asserted
    /// against the artefact rather than argued from the format's documentation.
    ///
    /// ⚠ SKIPPED, loudly, when the daemon has not been built in this profile: this crate takes no
    /// dependency on the product and must not require it to be built. The skip prints, so it cannot
    /// be a silent green.
    ///
    /// ⚠⚠ AND THE SKIP IS DECIDED BY THE RECORD, NOT BY THE BINARY — they are two artefacts and
    /// they come and go SEPARATELY. `cargo test --workspace` in a fresh target dir uplifts
    /// `sprag-term` without writing `sprag-term.d` beside it, so a guard that asked
    /// `bin.is_file()` said *"built, assert away"* and the read then failed with a bare
    /// `NotFound` — a red about a missing depfile, in a gate whose whole subject is what that
    /// depfile CONTAINS. Measured on a fresh worktree: binary present at 96 MB, no record, and
    /// `cargo build -p sprag-host --bins` turned the same run green.
    ///
    /// The fix keeps the claim rather than skipping the case: the guard now names the artefact the
    /// assertion actually reads. A record that IS there is still asserted on, exactly as before.
    #[test]
    fn cargos_own_record_for_this_workspaces_daemon_spans_the_whole_closure() {
        let Some(record) = built_daemon_record() else {
            // ⚠ The remedy is derived here for the same reason the refusals above are — item 455.
            // A skip that names the wrong command is a dead end with nobody watching, because a
            // skip is not a red.
            eprintln!(
                "skipped: no sprag-term depfile in target/debug — run `{}` (`cargo test` alone \
                 uplifts the binary without writing one)",
                owners::build_command(Path::new("sprag-term")),
            );
            return;
        };
        let mut crates: Vec<&str> = record
            .split_whitespace()
            .filter_map(|path| path.split("crates/").nth(1))
            .filter_map(|rest| rest.split('/').next())
            .collect();
        crates.sort_unstable();
        crates.dedup();
        assert!(
            crates.len() > 1,
            "a record naming one crate would make this check blind to every dependency: {crates:?}",
        );
        assert!(
            crates.contains(&"sprag-host"),
            "...and it must name the crate the binary is IN: {crates:?}",
        );
    }

    /// The shipped daemon in this workspace's debug profile, or `None` when nobody has built it.
    fn built_daemon() -> Option<PathBuf> {
        let bin = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()?
            .parent()?
            .join("target/debug/sprag-term");
        bin.is_file().then_some(bin)
    }

    /// What cargo recorded beside that daemon, or `None` when there is no record to read.
    ///
    /// Separate from [`built_daemon`] because the binary and its depfile are separate artefacts
    /// with separate lifetimes — see the caller. Anything asking *"can I assert on the record?"*
    /// must ask THIS.
    fn built_daemon_record() -> Option<String> {
        std::fs::read_to_string(built_daemon()?.with_extension("d")).ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A directory per test, since these run as threads of one binary.
    fn scratch(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "sprag-gate-{}-{tag}-{:?}",
            std::process::id(),
            std::thread::current().id(),
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("a scratch home");
        dir
    }

    #[test]
    fn a_home_nobody_wrote_to_reports_nothing() {
        let home = scratch("clean");
        assert_eq!(
            writes_under(&home).expect("walk a clean home"),
            Vec::<PathBuf>::new()
        );
    }

    /// ⚠ THE ONE THE SHIPPED SHELL COULD NOT MAKE. The write this guard exists to catch is
    /// `<config home>/sprag/config.toml`, so a walk that stops at the first level is blind to it.
    #[test]
    fn a_file_the_suite_left_two_levels_down_is_found() {
        let home = scratch("nested");
        std::fs::create_dir_all(home.join("sprag")).expect("the product's own directory");
        std::fs::write(
            home.join("sprag").join("config.toml"),
            "window-size = \"manual\"\n",
        )
        .expect("the file R341 measured a test writing");

        let found = writes_under(&home).expect("walk a written home");
        assert!(
            found.contains(&home.join("sprag").join("config.toml")),
            "the file two levels down must be named: {found:?}",
        );
        // And the directory holding it, so a report says the whole shape of what appeared.
        assert!(found.contains(&home.join("sprag")), "{found:?}");
    }

    /// A home that cannot be read is an ERROR, never an empty walk.
    ///
    /// This is the class the shipped guard belonged to from the other side: it looked one level too
    /// high and could never be quiet. The opposite mistake — looking somewhere that does not exist —
    /// is quiet FOREVER, which is worse, because a gate that always passes reads exactly like a
    /// product that is behaving.
    #[test]
    fn a_home_that_is_not_there_is_a_failure_and_not_a_pass() {
        let home = scratch("absent").join("never-created");
        let error = writes_under(&home).expect_err("a walk that cannot start must say so");
        assert_eq!(error.kind(), std::io::ErrorKind::NotFound);
    }

    #[test]
    fn an_unset_variable_is_named_rather_than_skipped() {
        let error = homes_from(|_| None).expect_err("nothing was set");
        assert_eq!(error, Unwatchable::Unset("XDG_CONFIG_HOME"));
        assert!(
            error.to_string().contains("the SAME three variables"),
            "and it says how to fix it: {error}",
        );
    }

    #[test]
    fn a_variable_pointing_at_a_file_is_named_with_what_it_pointed_at() {
        let dir = scratch("not-a-dir");
        let file = dir.join("this-is-a-file");
        std::fs::write(&file, "x").expect("write the decoy");
        let owned = file.clone();

        let error = homes_from(move |var| {
            (var == "XDG_CONFIG_HOME").then(|| OsString::from(owned.clone()))
        })
        .expect_err("a file is not a home");
        match error {
            Unwatchable::Unreadable { var, path, .. } => {
                assert_eq!(var, "XDG_CONFIG_HOME");
                assert_eq!(path, file);
            }
            other => panic!("the file must be named, not {other:?}"),
        }
    }

    /// All three are required, not just the first — the guard's claim covers config, data and state.
    #[test]
    fn every_home_the_list_names_must_be_watchable() {
        let dir = scratch("partial");
        let owned = dir.clone();
        let error =
            homes_from(move |var| (var != "XDG_STATE_HOME").then(|| OsString::from(owned.clone())))
                .expect_err("one missing home is a missing guard");
        assert_eq!(error, Unwatchable::Unset("XDG_STATE_HOME"));
    }
}
