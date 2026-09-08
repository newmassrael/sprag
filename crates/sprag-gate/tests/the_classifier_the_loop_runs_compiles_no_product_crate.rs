//! 🎯🎯🎯🎯🎯 **THE LOOP'S CLASSIFIER IS BUILT FROM ITS OWN CRATE AND NOTHING ELSE** — register
//! item 841, and the gate its `Done when` ⑵ asks for.
//!
//! # ⛔⛔⛔⛔⛔ What was true by accident until this ran
//!
//! `debt_loop.scxml` authors a `successor_check` — the program that says whether a proposal is one
//! this repository wants worked on next — and it is a `cargo run`. Item 841 was opened believing
//! that made re-aiming depend on the whole workspace compiling. It does not, and the reason is one
//! line in one manifest: the classifier's package declares no dependencies, so cargo compiles that
//! package alone. **Nothing held anybody to that line.** It was a comment.
//!
//! Measured 2026-09-08, in a throwaway worktree, what the comment is worth: adding one
//! `sprag-detect = { workspace = true }` to that `[dependencies]` took the closure from **one crate
//! to a hundred** — `proc-macro2`, `syn`, `regex`, `termwiz`, `sprag-vt` and the rest — every one
//! of them then able to leave a round unable to re-aim. That edit passes every other gate in this
//! workspace.
//!
//! # ⚠⚠⚠ Why this drives cargo instead of walking `[dependencies]`
//!
//! A walk over the manifest would be this gate's reading of the graph, and three more sections
//! reach a `--bin` build than the obvious one. Cargo prints what it really compiled, so the closure
//! is read off a run — see [`sprag_gate::classifier::compiled_crates`], which also says why the
//! target directory has to be EMPTY.
//!
//! ⚠ It builds WITHOUT the document's `-q`, deliberately: that switch changes what cargo PRINTS
//! and not what it compiles, and the printing is the whole evidence here.
//!
//! ⚠⚠ And it builds the package and binary the document NAMES, in the tree under test — not at the
//! absolute manifest path the document spells. That path is this machine's; the package name is not
//! and neither is the claim.

use sprag_gate::classifier::{self, Deployed, KIND_DOCUMENT, LOCKED};
use sprag_gate::sources::workspace_root;
use std::process::Command;

/// A crate that is NOT the classifier's, used as the control arm's subject.
///
/// ⚠ Chosen for two properties rather than at random: it declares no dependencies of its own, so
/// the control costs one crate like the claim does, and it is a workspace MEMBER — a control over
/// some registry crate would leave *is a sibling of the classifier visible to this reader* unasked.
const A_CRATE_THAT_IS_NOT_THE_CLASSIFIERS: &str = "sprag-scratch";

/// The classifier this repository's kind document deploys, or a refusal that says which part of it
/// could not be read.
fn deployed() -> Deployed {
    let path = workspace_root().join(KIND_DOCUMENT);
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|why| panic!("read the kind document at {}: {why}", path.display()));
    classifier::deployed_classifier(&text).unwrap_or_else(|why| {
        panic!(
            "⛔⛔⛔ ITEM 841: this repository's own classifier could not be read out of {}, so \
             nothing below is about the program the loop runs — {}",
            path.display(),
            why.describe(),
        )
    })
}

/// A scratch path no other run of this suite shares.
fn somewhere_empty(prefix: &str) -> std::path::PathBuf {
    let tail = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |since| since.subsec_nanos());
    let path = sprag_scratch::scratch_for(prefix, &format!("{tail}"));
    // ⚠ Cargo prints nothing for a unit it did not have to build, so a directory left by an
    // earlier run of this test would answer *nothing was compiled* about a closure that is
    // whatever it is. Emptying it is the staging, and `compiled_crates` refuses the log either way.
    let _ = std::fs::remove_dir_all(&path);
    path
}

/// Run cargo with `args` against the TREE UNDER TEST and hand back everything it said.
///
/// ⚠ `current_dir` is the only thing here that names a tree, deliberately: cargo discovers the
/// workspace from where it stands, and `workspace_root` is the reader that refuses when the tree
/// compiled in and the tree being run in disagree (register item 809). A `--manifest-path` spelled
/// beside it would be a second authority on the same question.
fn built(args: &[&str], into: &std::path::Path) -> (bool, String) {
    let run = Command::new(env!("CARGO"))
        .args(args)
        .env("CARGO_TARGET_DIR", into)
        .current_dir(workspace_root())
        .output()
        .unwrap_or_else(|why| panic!("cargo runs `{}`: {why}", args.join(" ")));
    let said = format!(
        "{}{}",
        String::from_utf8_lossy(&run.stdout),
        String::from_utf8_lossy(&run.stderr),
    );
    (run.status.success(), said)
}

/// ⛔⛔⛔⛔⛔ **NO PRODUCT CRATE IS IN THE CLASSIFIER'S COMPILE CLOSURE** — item 841's ⑵, and the
/// general form of *break the product and see*: a closure of one cannot be broken by any of them,
/// rather than by the one somebody thought to break.
#[test]
fn the_classifier_the_loop_runs_compiles_no_product_crate() {
    let read = deployed();
    let empty = somewhere_empty("sprag-classifier-closure");

    let (ok, log) = built(
        &["build", LOCKED, "-p", &read.package, "--bin", &read.bin],
        &empty,
    );
    // ⚠ THE STAGING, NOT THE CLAIM. A build that did not finish says nothing about a closure, and
    // everything below would be about whatever cargo printed on its way out.
    assert!(
        ok,
        "⚠⚠ the classifier's own binary must build before this gate can say anything about what \
         it is built FROM. `cargo build -p {} --bin {}` failed:\n{log}",
        read.package, read.bin,
    );

    let compiled = classifier::compiled_crates(&log)
        .unwrap_or_else(|why| panic!("⚠⚠ THE STAGING: {}\n{log}", why.describe()));
    let foreign = classifier::foreign_to(&compiled, &read.package);
    assert!(
        foreign.is_empty(),
        "⛔⛔⛔ ITEM 841: the program that decides where this loop may aim is compiled from {} \
         crate(s) that are not its own, and a red in ANY of them leaves a round unable to re-aim \
         — a proposal nothing could classify is SET ASIDE, so the loop keeps its brief for ever. \
         `{}` declares no dependencies for exactly this reason; put back whatever was added to \
         its `[dependencies]`. Cargo compiled, beyond the classifier itself: {}",
        foreign.len(),
        read.package,
        classifier::named(&foreign),
    );

    // ── AND THE CONTROL, FROM A REAL LOG AND NOT A FIXTURE ────────────────────────────────────
    //
    // ⚠⚠⚠ A reader that answered *nothing foreign* for every log would pass the assertion above
    // no matter what the manifest said, which is the vacuous green this workspace keeps meeting
    // (register item 799). So the same reader is shown a build of a DIFFERENT member, by the same
    // cargo, into the same directory — and it has to name it.
    let (control_ok, control) = built(
        &[
            "build",
            LOCKED,
            "-p",
            A_CRATE_THAT_IS_NOT_THE_CLASSIFIERS,
            "--lib",
        ],
        &empty,
    );
    assert!(
        control_ok,
        "⚠ the control's staging: {A_CRATE_THAT_IS_NOT_THE_CLASSIFIERS} must build for its log to \
         be a log:\n{control}",
    );
    let control_closure = classifier::compiled_crates(&control)
        .unwrap_or_else(|why| panic!("⚠⚠ THE CONTROL'S STAGING: {}\n{control}", why.describe()));
    assert!(
        classifier::foreign_to(&control_closure, &read.package)
            .contains(A_CRATE_THAT_IS_NOT_THE_CLASSIFIERS),
        "⛔⛔⛔⛔⛔ THE CONTROL: cargo really compiled \
         {A_CRATE_THAT_IS_NOT_THE_CLASSIFIERS} in this very log and this reader does not see it, \
         so the claim above is green about nothing. Cargo said: {control_closure:?}",
    );
}

/// ⛔⛔⛔⛔⛔ **THE CLASSIFIER CANNOT REWRITE THE LOCKFILE OF THE TREE IT IS JUDGING** — item 841,
/// and the one hazard the register's own headline did not name.
///
/// # ⛔⛔⛔ Measured, because it had already happened
///
/// A `cargo run` re-resolves whenever a manifest has moved past `Cargo.lock`, and re-resolving
/// WRITES. 2026-09-08, in a throwaway worktree: one path dependency added to one member, and the
/// classifier answered `YES STEP` and rewrote the lockfile in the same second — digest `82c008d5…`
/// before, `dfa09b73…` after, offline, exit 0. Register item 196 is that this tree has two writers,
/// so that rewrite lands in whatever the agent commits next with nobody having read it.
///
/// With [`LOCKED`] the same run exits 101, the digest does not move, and cargo's own first line
/// says why — which is the line `Admits::Silent` carries into the run's row.
///
/// ⚠ The trade this pins, stated rather than hidden: a stale lock now costs this run its
/// classifier. Silence sets a proposal aside and keeps the brief, which is the safe direction; a
/// rewritten lockfile is not recoverable by keeping a brief.
#[test]
fn the_classifier_cannot_rewrite_the_lockfile_of_the_tree_it_judges() {
    let read = deployed();
    assert!(
        read.cargo_flags.contains(LOCKED),
        "⛔⛔⛔ ITEM 841: this classifier is a `cargo run` over the tree the agent it judges is \
         editing, and without {LOCKED} it rewrites that tree's `Cargo.lock` the moment a manifest \
         moves ahead of it — measured, with the digests, in this test's doc. Cargo switches read \
         off the document: {:?}",
        read.cargo_flags,
    );
}

/// ⚠⚠⚠ **AND IT BUILDS SOMEWHERE THE AGENT IT JUDGES IS NOT BUILDING** — register item 196, which
/// item 841's own measurement table records as `target/` 경합 — 없다 and which was, until here, a
/// sentence in a comment.
///
/// Cargo's default target directory is the tree's own `target/`, and this tree's is shared with the
/// agent this loop drives. A classifier queued behind that agent's build outruns
/// `ADMITS_WITHIN` and answers *silent*, which sets every proposal aside — the same ending as a
/// broken classifier, from a cause nobody would look for. So the environment prefix naming a
/// directory of its own is part of the argv's claim, and dropping it is a red.
#[test]
fn the_classifier_builds_somewhere_the_agent_it_judges_is_not() {
    let read = deployed();
    let mine = workspace_root();
    let Some(theirs) = read.target_dir.as_ref() else {
        panic!(
            "⛔⛔⛔ ITEM 196: this classifier names no `CARGO_TARGET_DIR`, so cargo will build it \
             into {}, which is the directory the agent this loop drives is building in. A \
             classifier that queues behind that build answers *silent*, and a silent classifier \
             sets aside every proposal the run meets. Argv: {:?}",
            mine.join("target").display(),
            read.argv,
        );
    };
    assert!(
        !theirs.starts_with(&mine),
        "⛔⛔⛔ ITEM 196: the classifier's target directory {} is inside the tree under test {}, \
         so it can contend with the very agent it is refereeing. It needs one of its own.",
        theirs.display(),
        mine.display(),
    );
}
