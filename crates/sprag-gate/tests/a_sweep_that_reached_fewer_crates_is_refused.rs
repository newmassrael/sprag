//! **THE GUARD REFUSES A REAL SHORT SWEEP AND PASSES A REAL WHOLE ONE** — register item 585.
//!
//! # ⚠⚠⚠⚠⚠ Why the log is cargo's and not this file's
//!
//! [`sprag_gate::sweep`]'s own unit tests hand `unreported` strings written here, which is right
//! for the parsing rule and says nothing about the thing that actually failed: whether the shape
//! **cargo really prints** is the shape the guard reads. A fixture authored beside the reader agrees
//! with it by construction — the failure this repository has paid for under several names, most
//! recently on 2026-08-23 when a JSON fixture written by hand hid a publication condition no daemon
//! could ever satisfy.
//!
//! So this drives `cargo test` for real, narrowly, and hands the guard what came out.
//!
//! # ⚠⚠ The two arms come out of ONE run
//!
//! The narrow run covers two crates and not the rest, so the same log is both the arm (thirteen
//! crates unreached, and the guard must say so) and the control (two crates reached, and the guard
//! must not name them). A whole sweep as a second arm would cost minutes and add nothing the pair
//! below does not already separate — and a guard that named everything, or nothing, fails one half
//! of this either way.
//!
//! ⚠ `--no-run` is deliberately NOT used: it compiles the tests and prints no `Running` line at
//! all, so it would exercise the guard against an artefact no sweep produces.

use std::path::PathBuf;

/// The workspace root — through the one door, register item 809.
///
/// ⚠ It spelled `env!("CARGO_MANIFEST_DIR")` and two `".."` of its own until 2026-09-01. That
/// answer is baked when the test is COMPILED, so a build whose output reached this tree from
/// another one made this gate judge somebody else's workspace without a word. `workspace_root`
/// compares the compiled-in tree against the one the run is standing in and refuses when they
/// differ; a private copy is a hole straight past that.
fn root() -> PathBuf {
    sprag_gate::sources::workspace_root()
}

/// ⚠⚠⚠ **THE GUARD IS DRIVEN AS THE PROGRAM A CALLER RUNS**, not as the function under it. Item
/// 585's whole shape is that a sweep's coverage is judged from OUTSIDE the sweep, and what a CI
/// step or a hook invokes is `cargo run --bin sweep-coverage` — its argument handling, its reading
/// of the manifest beside its own source, and its EXIT CODE are the parts a caller depends on, and
/// none of them are exercised by calling `unreported`.
#[test]
fn a_sweep_that_ran_two_crates_is_refused_and_told_which_ones_it_missed() {
    // A real, narrow sweep. Two crates with no dependencies of their own, so this stays seconds
    // rather than minutes — `sprag-vt` is the workspace's bottom crate and `sprag-grid` sits beside
    // it. Its own success is not the claim and is not asserted: what matters is the LOG.
    let narrow = std::process::Command::new(env!("CARGO"))
        .args([
            "test",
            "-p",
            "sprag-vt",
            "-p",
            "sprag-grid",
            "--lib",
            "--no-fail-fast",
        ])
        .current_dir(root())
        .output()
        .expect("cargo runs a narrow sweep");
    let log = format!(
        "{}{}",
        String::from_utf8_lossy(&narrow.stdout),
        String::from_utf8_lossy(&narrow.stderr),
    );
    assert!(
        log.contains("deps/sprag_vt-"),
        "⚠ THE STAGING, NOT THE CLAIM: this arm needs a log from a run that really happened, and \
         cargo printed no `Running` line for a crate it was asked for. Everything below would be \
         about an empty string instead of about a sweep. Got:\n{log}",
    );

    // The log goes to a file, because that is what the guard takes and what a sweep leaves behind.
    let kept = std::env::temp_dir().join(format!("sprag-sweep-{}.log", std::process::id()));
    std::fs::write(&kept, &log).expect("the narrow sweep's log is kept for the guard");

    let judged = std::process::Command::new(env!("CARGO"))
        .args(["run", "-q", "-p", "sprag-gate", "--bin", "sweep-coverage"])
        .arg("--")
        .arg(&kept)
        .current_dir(root())
        .output()
        .expect("the guard runs");
    let said = String::from_utf8_lossy(&judged.stderr).into_owned();

    assert!(
        !judged.status.success(),
        "⛔⛔⛔ ITEM 585: this sweep touched two crates out of fifteen and the guard exited 0. On \
         2026-08-22 a sweep nineteen crates short reported success, and the only thing that caught \
         it was somebody counting lines by hand — an exit code is what a round actually reads. It \
         said: {said}{}",
        String::from_utf8_lossy(&judged.stdout),
    );
    for missed in ["sprag-host", "sprag-plugin", "sprag-terminal"] {
        assert!(
            said.contains(missed),
            "⚠⚠⚠ AND IT NAMES THEM, because *some crates did not run* sends a person to count \
             what a machine already knows. {missed:?} is not in: {said}",
        );
    }
    for reached in ["sprag-vt", "sprag-grid"] {
        assert!(
            !said.contains(reached),
            "⚠⚠⚠⚠⚠ THE CONTROL: cargo really ran {reached} in this very log, and the guard names \
             it as unreached. A guard that names every crate refuses every sweep there is, which \
             is a gate somebody switches off rather than fixes: {said}",
        );
    }

    // ── AND A GAP THE CALLER DECLARED IS NOT A FINDING ──
    //
    // ⚠⚠⚠ **THE HONEST SWEEP IS THE COMMON PATH AND HAS TO PASS.** CI's headless job really does
    // leave the GPU crate to another runner, so a guard with no way to be told that would refuse
    // every real run — and a gate that is wrong on the common path is one people route around
    // rather than fix. The same log, the same guard, every unreached crate declared: exit 0.
    let mut declared = vec![
        "run".to_owned(),
        "-q".to_owned(),
        "-p".to_owned(),
        "sprag-gate".to_owned(),
        "--bin".to_owned(),
        "sweep-coverage".to_owned(),
        "--".to_owned(),
        kept.to_string_lossy().into_owned(),
    ];
    let members = sprag_gate::sweep::members(
        &std::fs::read_to_string(root().join("Cargo.toml")).expect("the root manifest"),
    );
    for crate_name in sprag_gate::sweep::unreported(&members, &log) {
        declared.push("--excluding".to_owned());
        declared.push(crate_name);
    }
    let told = std::process::Command::new(env!("CARGO"))
        .args(&declared)
        .current_dir(root())
        .output()
        .expect("the guard runs again");
    let _ = std::fs::remove_file(&kept);
    assert!(
        told.status.success(),
        "⚠⚠⚠⚠ EVERY CRATE THIS SWEEP MISSED WAS NAMED ON THE COMMAND, so there is nothing left \
         for the guard to find and it must pass. Refusing here would make the flag decorative and \
         the gate unusable by the job it was built for: {}{}",
        String::from_utf8_lossy(&told.stderr),
        String::from_utf8_lossy(&told.stdout),
    );
}
