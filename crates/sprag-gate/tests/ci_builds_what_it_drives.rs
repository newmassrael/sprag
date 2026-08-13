//! **A JOB THAT RUNS THE SUITE MUST BUILD THE BINARIES THE SUITE DRIVES** — the claim `ci.yml`
//! could not make about itself.
//!
//! # ⚠⚠⚠ Why this file exists
//!
//! [`sprag_gate::sibling_bin`] answers *was this binary built from the source in this tree* by
//! reading cargo's own depfile beside it, and it refuses to pass when it cannot tell. That refusal
//! is right: the alternative is a suite that drives a stale daemon and reports green, which this
//! workspace has paid for three times.
//!
//! What nobody measured is where that depfile comes from. **`cargo test` UPLIFTS
//! `target/debug/sprag-term` and writes no `sprag-term.d` beside it; only `cargo build` writes the
//! uplifted depfile.** So a CI job whose only cargo step is `cargo test` leaves the guard with
//! nothing to read, and the guard correctly refuses the entire pty and MCP suite — which is what
//! the Linux job did in the commit that introduced the guard, while the macOS job (which has always
//! had a build step) failed for an unrelated reason and hid the pattern.
//!
//! # ⚠⚠ Why a LINE SCAN and not a YAML parse, said plainly
//!
//! This crate takes no dependencies by charter, and there is no YAML reader in std. A scan cannot
//! understand `ci.yml`; what it can do is answer one narrow question — *does the text of this job
//! invoke `cargo build` before it invokes `cargo test`* — and that is the whole claim. It is
//! stated here rather than implied so nobody reads a green run as *the workflow is correct*.
//!
//! ⚠ It is also why the job boundary is found by INDENTATION rather than by structure: a job's key
//! sits at exactly four spaces under `jobs:`, and anything deeper belongs to it.

use std::path::PathBuf;

/// The workflow this repository's gates run in.
fn workflow() -> String {
    let path: PathBuf = [
        env!("CARGO_MANIFEST_DIR"),
        "..",
        "..",
        ".github",
        "workflows",
        "ci.yml",
    ]
    .iter()
    .collect();
    std::fs::read_to_string(&path)
        .unwrap_or_else(|why| panic!("{} is the workflow under test: {why}", path.display()))
}

/// Every job in `ci.yml`, as `(name, its own lines)`.
///
/// A job's key is indented exactly four spaces under `jobs:`; every deeper line is that job's until
/// the next such key. Comments and blanks travel with whichever job they sit in, which is harmless
/// here — a commented-out `cargo build` would be a false positive, so they are stripped.
fn jobs(text: &str) -> Vec<(String, Vec<String>)> {
    let mut found: Vec<(String, Vec<String>)> = Vec::new();
    let mut in_jobs = false;
    for line in text.lines() {
        if line.starts_with("jobs:") {
            in_jobs = true;
            continue;
        }
        if !in_jobs {
            continue;
        }
        let indent = line.len() - line.trim_start().len();
        let trimmed = line.trim_start();
        if trimmed.is_empty() {
            continue;
        }
        // A top-level key ends the `jobs:` block entirely.
        if indent == 0 {
            in_jobs = false;
            continue;
        }
        if indent == 2 && trimmed.ends_with(':') && !trimmed.starts_with('#') {
            found.push((trimmed.trim_end_matches(':').to_owned(), Vec::new()));
            continue;
        }
        // ⚠ COMMENTS ARE DROPPED. This file explains itself at length, and several of those
        // paragraphs quote the very commands being searched for — so a scan that read them would
        // find `cargo build` in prose and pass a job that runs none.
        if trimmed.starts_with('#') {
            continue;
        }
        if let Some((_, lines)) = found.last_mut() {
            lines.push(trimmed.to_owned());
        }
    }
    found
}

/// ⚠⚠⚠ **EVERY JOB THAT RUNS `cargo test` RUNS `cargo build` FIRST.**
///
/// Not a style rule. The suite spawns `sprag-term` and `sprag-tui` as PROCESSES, and
/// [`sprag_gate::sibling_bin`] refuses a binary whose build record is missing — which `cargo test`
/// alone does not write. A job without the build step does not run a weaker suite; it runs NO
/// suite, and says so in a hundred identical panics.
///
/// ⚠ THE ORDER is part of the claim: a build AFTER the test proves nothing about the test.
#[test]
fn every_job_that_runs_the_suite_builds_the_binaries_it_drives() {
    let text = workflow();
    let mut checked = 0_usize;
    for (name, lines) in jobs(&text) {
        let Some(tests_at) = lines.iter().position(|line| line.contains("cargo test")) else {
            continue;
        };
        checked += 1;
        let builds_at = lines
            .iter()
            .position(|line| line.contains("cargo build"))
            .unwrap_or_else(|| {
                panic!(
                    "⚠⚠⚠ the {name:?} job runs `cargo test` and never runs `cargo build`. The \
                     suite drives sibling binaries as processes and `sprag_gate::sibling_bin` \
                     reads cargo's depfile to answer whether they are current — a depfile only \
                     `cargo build` writes beside an uplifted binary. Without that step the guard \
                     cannot tell, refuses correctly, and the whole pty and MCP suite fails on this \
                     runner alone."
                )
            });
        assert!(
            builds_at < tests_at,
            "⚠⚠ the {name:?} job builds AFTER it tests, which is the same as not building: the \
             depfile the guard reads has to exist when the suite starts, not when it is over",
        );
    }
    // THE CONTROL. Every assertion above is satisfied by a scan that found no jobs at all — which
    // is exactly what a changed indentation or a renamed `jobs:` key would produce.
    assert!(
        checked >= 2,
        "this scan found only {checked} job(s) running the suite, and this workflow has more than \
         one. The parse has stopped seeing the file rather than the file having changed.",
    );
}
