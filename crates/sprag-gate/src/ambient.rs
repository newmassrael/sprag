//! The git environment a suite must not hand a child — register item 965.
//!
//! # ⛔⛔⛔⛔⛔ What it costs, measured
//!
//! `git commit -- <pathspec>` is a PARTIAL commit: git builds a TEMPORARY index, hands its hooks
//! an **absolute** `GIT_INDEX_FILE` naming it, and commits whatever that file holds when the hooks
//! return. `pre-commit` here runs `cargo test -p sprag-gate`, so every child this suite starts
//! inherits that variable — and it outranks `Command::current_dir`, `git -C` and repository
//! discovery all three. A sandbox doing `git add` in its own throwaway repository therefore writes
//! the entry into the index git is about to commit.
//!
//! Measured 2026-09-08, in a throwaway repository so the numbers are this machine's: three of the
//! five `.githooks/` selftests took a caller's index from **2** entries to **4**, **5** and **7**,
//! and `hooks_judge_the_bytes_being_published` — twenty-one sandboxes, run in parallel — collided
//! on the caller's `index.lock` and failed with *"another git process seems to be running"*. Where
//! the scratch's blobs happened to exist already the outer commit SUCCEEDED and published the
//! sandbox's content: four such commits reached `main`, one of them replacing `.gitignore` with a
//! single line.
//!
//! # ⚠⚠ Why the whole namespace and not the one variable
//!
//! A list of names to strip passes every variable nobody has thought of yet, and the variable that
//! cost those four commits was already unlisted by the scratch guard that exists for exactly this
//! failure. A suite that builds its own repository needs NO inherited git environment: git finds
//! its exec path, its config, its object store and its index from the directory it is handed. So
//! the population is the namespace, and there is no exemption list to keep current.
//!
//! # ⚠ Why it is here and not in each suite
//!
//! This crate's own reason, one file over: the claim is about what a SUITE DID to something
//! outside itself, and a rule kept in one test file is a rule the next one does not get.
//! `.githooks/scratch-guard.sh` holds the same decision for the shell harnesses, and
//! `no_suite_hands_a_child_the_git_environment_it_inherited` is what keeps a new call site from
//! going around both.

use std::ffi::OsString;
use std::path::Path;
use std::process::Command;

/// Whether `name` belongs to git's own environment — the classification the cut is over.
///
/// ⚠⚠ THE WHOLE NAMESPACE. See this module's header for why a list of the dangerous names is the
/// shape that already failed: `GITHUB_TOKEN` and friends are outside it, and a sandbox that lost
/// `PATH` or `HOME` would fail in a way nobody could read off the failure.
#[must_use]
pub fn is_git_environment(name: &str) -> bool {
    name.starts_with("GIT_")
}

/// Every git name this process would hand a child, sorted — the population the cut is over.
///
/// ⚠ EXPORTED variables only, because that is all a child can read. This is the environment as it
/// stands when it is asked, so a caller that has already cut gets an empty list.
#[must_use]
pub fn inherited_git_names() -> Vec<OsString> {
    let mut found: Vec<OsString> = std::env::vars_os()
        .map(|(name, _)| name)
        .filter(|name| is_git_environment(&name.to_string_lossy()))
        .collect();
    found.sort();
    found
}

/// Take `names` out of what `run` will hand its child.
///
/// ⚠⚠ SEPARATE FROM [`git_in`] SO IT CAN BE DRIVEN. The cut's whole subject is what a CHILD
/// receives, and the only honest way to measure that is to spawn one — but the names [`git_in`]
/// removes are this PROCESS's, and a test that set one would set it for every case running beside
/// it on another thread. Handed its list, the same operation is measurable against a real child.
pub fn cut<'a>(run: &'a mut Command, names: &[OsString]) -> &'a mut Command {
    for name in names {
        run.env_remove(name);
    }
    run
}

/// `git`, to be run in `at`, carrying none of the git environment this process inherited.
///
/// ⚠⚠ THE CONSTRUCTOR RATHER THAN A FUNCTION THAT TAKES ONE. A caller handed a built `Command` can
/// forget to pass it through, and the forgetting is silent; a caller that cannot name `git` any
/// other way — which is what
/// `tests/no_suite_hands_a_child_the_git_environment_it_inherited.rs` enforces — cannot forget.
///
/// ⚠ It removes, and sets nothing. A sandbox that wants `HOME` or `GIT_CONFIG_NOSYSTEM` of its own
/// adds them afterwards, and those additions survive: the removals are applied here, before the
/// caller's builder calls. A variable a caller names DELIBERATELY is not one this took away.
pub fn git_in(at: &Path) -> Command {
    let mut run = Command::new("git");
    run.current_dir(at);
    cut(&mut run, &inherited_git_names());
    run
}

#[cfg(test)]
mod tests {
    use super::*;

    /// ⛔⛔ THE CLASSIFICATION, DRIVEN DIRECTLY — and it is a pure function for a reason this
    /// workspace has already paid for: the live reading answers about THIS process, whose
    /// environment a test cannot change without changing it for every case running beside it on
    /// another thread. The behaviour that matters — that a child really does not receive what was
    /// cut — is measured where a scratch directory exists to measure it in, by
    /// `tests/no_suite_hands_a_child_the_git_environment_it_inherited.rs`.
    #[test]
    fn the_namespace_is_the_population_and_nothing_wider() {
        for named in [
            "GIT_INDEX_FILE",
            "GIT_DIR",
            "GIT_WORK_TREE",
            "GIT_OBJECT_DIRECTORY",
            "GIT_CONFIG_GLOBAL",
            "GIT_",
        ] {
            assert!(
                is_git_environment(named),
                "{named} is git's own environment and a scratch harness must not inherit it",
            );
        }
        for outside in [
            "GITHUB_TOKEN",
            "PATH",
            "HOME",
            "TMPDIR",
            "SPRAG_GIT_DIR",
            "git_dir",
        ] {
            assert!(
                !is_git_environment(outside),
                "⛔ {outside} is not git's environment, and a cut that swept it up would break a \
                 sandbox for a reason nobody could read off the failure",
            );
        }
    }

    /// ⚠ THE LIVE READING IS THAT CLASSIFICATION APPLIED TO THIS PROCESS AND NOTHING ELSE, so every
    /// name it returns is one the classification accepts. Asserted rather than assumed: the two
    /// could drift into different filters, and only the pure one is driven above.
    #[test]
    fn every_name_the_live_reading_returns_is_one_the_classification_accepts() {
        for name in inherited_git_names() {
            assert!(
                is_git_environment(&name.to_string_lossy()),
                "the live reading returned {name:?}, which the classification refuses — the two \
                 have drifted, and only one of them is measured",
            );
        }
    }
}
