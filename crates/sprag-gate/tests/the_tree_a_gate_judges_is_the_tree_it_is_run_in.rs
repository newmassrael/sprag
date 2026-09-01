//! ⛔⛔⛔⛔⛔ **THE TREE A GATE JUDGES IS THE TREE IT IS RUN IN** — register item 809.
//!
//! # What was wrong: a compile-time answer to a run-time question
//!
//! Every tree-reading gate in this workspace started from `env!("CARGO_MANIFEST_DIR")`, which rustc
//! expands when the crate is COMPILED. That makes the walked root a fact about the build. On this
//! machine a build's output can reach a second tree — `~/.cargo/config.toml` sets
//! `rustc-wrapper = "sccache"` with the stated purpose *"Share compiled crates across every
//! `target/` directory on this box"*, and `sprag-host`'s own checker cuts throwaway `git worktree`s
//! under the temporary directory — so *the tree this gate judges* and *the tree somebody is running
//! it in* became two facts sharing one sentence.
//!
//! ⚠⚠ MEASURED 2026-09-01: eight arms of `cargo test -p sprag-gate` died at once reading
//! `/tmp/sprag-check-2151720-916922480/…`, a worktree that no longer existed. Recompiling this
//! crate from this tree — nothing else changed — turned all 28 targets green.
//!
//! ⚠⚠⚠ AND THE LOUD FAILURE IS THE LUCKY HALF. A worktree that still exists makes the walk read a
//! real tree that is somebody else's, silently, and report green about a workspace nobody asked
//! about. That is why this gate asserts the AGREEMENT rather than waiting for a missing file.
//!
//! # ⚠⚠⚠⚠ Why the ratchet arm is here too
//!
//! A door only helps while everything goes through it. Seven test files in this crate and one in
//! `sprag-host` each spelled the walk privately, and each was a hole straight past the check — the
//! escape-hatch shape this repository refuses on principle: *what is not classified is RED, not a
//! pass.* So the population is walked and the spelling is forbidden outside the one file that owns
//! it.

use sprag_gate::sources::{TreeUnderTest, rust_sources, tree_skew_sentence, workspace_root};
use std::path::PathBuf;

/// The one file allowed to spell the compile-time root, and why.
///
/// ⚠ It is a `(file, why)` pair rather than a bare path for this workspace's usual reason: an
/// exemption whose reason is not written down is one nobody can judge later.
const OWNS_THE_SPELLING: (&str, &str) = (
    "crates/sprag-gate/src/sources.rs",
    "the door itself -- it holds one half of the comparison and is the only place that may",
);

/// This gate's own source, which quotes the forbidden spelling in order to hunt for it.
const QUOTES_ITSELF: (&str, &str) = (
    "crates/sprag-gate/tests/the_tree_a_gate_judges_is_the_tree_it_is_run_in.rs",
    "the needle has to be written to be searched for, and splitting it so it stops matching \
     itself is the trick that quietly stops matching anything",
);

/// The population gate, whose vocabulary names the shape in prose and in a constant.
const NAMES_THE_SHAPE: (&str, &str) = (
    "crates/sprag-gate/tests/a_ratchet_that_reads_the_tree_runs_before_the_commit_lands.rs",
    "it CLASSIFIES readers by this very spelling, so its vocabulary must contain it",
);

/// ── 1. THE LIVE VERDICT ──────────────────────────────────────────────────────────────────────
///
/// The tree this suite is judging is the tree it is standing in, and it is a tree that really
/// holds this gate's own source.
///
/// ⚠⚠ THE SECOND HALF IS NOT DECORATION. `Agreed` says the two derivations match; it does not say
/// the answer is a sprag checkout at all. Asking the root for THIS FILE, and for a sentence only
/// this file carries, makes the claim answerable from the artefact rather than from the pair of
/// paths that produced it — which is item 809's own done-when.
#[test]
fn the_tree_this_suite_judges_holds_this_gates_own_source() {
    let verdict = sprag_gate::sources::tree_under_test();
    let TreeUnderTest::Agreed(root) = verdict.clone() else {
        panic!("{}", tree_skew_sentence(&verdict));
    };
    assert_eq!(
        root,
        workspace_root(),
        "the door and the verdict must name one tree, or a caller and this gate disagree",
    );

    let mine = root.join(QUOTES_ITSELF.0);
    let text = std::fs::read_to_string(&mine).unwrap_or_else(|why| {
        panic!(
            "⛔ ITEM 809: the tree this suite walked ({}) does not carry this gate's own source at \
             {}: {why}. A root that cannot produce the file asking the question is not this \
             workspace, whatever the two path derivations agreed about",
            root.display(),
            mine.display(),
        )
    });
    assert!(
        text.contains("THE TREE A GATE JUDGES IS THE TREE IT IS RUN IN"),
        "⛔ ITEM 809: {} exists in the walked tree but is not this file, so the walk found a \
         DIFFERENT checkout that happens to have the same layout",
        mine.display(),
    );
}

/// ── 2. THE CLASSIFIER ANSWERS THREE STATES, AND SAYS THEM DIFFERENTLY ────────────────────────
///
/// Driven through the injected seam, because a machine will not produce a skew on demand — the
/// same reason `sprag_scratch`'s root split and item 802's `XdgHome` split exist.
///
/// ⚠ All three in one test on purpose: the defect was a COLLAPSE, and a property about a collapse
/// is only visible with the arms beside each other.
#[test]
fn a_tree_that_agrees_one_that_is_skewed_and_one_that_cannot_be_named_are_three_answers() {
    // ⚠⚠ TWO DIRECTORIES THAT HAVE NOTHING TO DO WITH THE SUBJECT, and that is the point. An
    // earlier draft used `workspace_root()` as the "here" of this fixture, which tied the arm to
    // the very function it is classifying: a mutation that made the door stop checking then failed
    // this arm for the wrong reason — a path spelled two ways, not a policy that folded — and a red
    // whose message is about something else is the same as no signal. Measured 2026-09-01.
    let here = PathBuf::from("/usr");
    let elsewhere = PathBuf::from("/tmp");

    assert_eq!(
        sprag_gate::sources::verdict_of(here.clone(), Ok(here.clone())),
        TreeUnderTest::Agreed(here.canonicalize().expect("the fixture directory resolves")),
        "one tree named twice is the only state in which a walk is a claim about it",
    );

    let skewed = sprag_gate::sources::verdict_of(here.clone(), Ok(elsewhere.clone()));
    assert!(
        matches!(skewed, TreeUnderTest::Skewed { .. }),
        "⚠ TWO DIFFERENT TREES MUST NOT FOLD INTO ONE ANSWER — that fold is the whole of item \
         809: {skewed:?}",
    );

    let unknown =
        sprag_gate::sources::verdict_of(here.clone(), Err("nothing above declares it".to_owned()));
    assert!(
        matches!(unknown, TreeUnderTest::Unknown { .. }),
        "⚠⚠ A ROOT THAT CANNOT BE ESTABLISHED IS ITS OWN STATE. Folding it into `Agreed` is an \
         escape hatch that opens exactly when the check is needed, and folding it into `Skewed` \
         would tell a reader two trees were compared when one was never found: {unknown:?}",
    );

    // ⚠⚠ AND THE THREE SENTENCES DIFFER. Two states that say the same words are two states nobody
    // can tell apart, which is the defect one level up.
    let said: Vec<String> = [&skewed, &unknown]
        .iter()
        .map(|verdict| tree_skew_sentence(verdict))
        .collect();
    assert!(
        said[0].contains(&here.display().to_string()) && said[0].contains("/tmp"),
        "⚠ a skew that does not name BOTH trees leaves the reader nothing to act on: {}",
        said[0],
    );
    assert!(
        said[0].contains("touch"),
        "⚠⚠ AND IT NAMES THE REPAIR. Nothing in the source is wrong when this fires, so the \
         ordinary instinct is to hunt for a source bug: {}",
        said[0],
    );
    assert_ne!(
        said[0], said[1],
        "the two refusals must not read alike: {said:?}",
    );
    assert!(
        !said[1].contains("/tmp"),
        "⚠ an unnameable root must not be reported as a comparison against a tree: {}",
        said[1],
    );
}

/// ── 3. THE DETECTOR FIRES END TO END, NOT ONLY IN ITS CLASSIFIER ─────────────────────────────
///
/// # ⛔⛔⛔⛔⛔ Why a classifier arm is not enough — register item 810
///
/// Arm 2 drives the policy with both answers injected, which proves the POLICY. It cannot prove
/// that the policy is wired to the real derivations, that `workspace_root` consults it, or that a
/// caller gets a refusal rather than a path. Item 809 measured a live skew and the repair was
/// verified by hand — running this suite's own binary from another tree and reading the sentence —
/// and a check that only a person can perform is one nobody performs twice.
///
/// ⚠⚠ THE SHAPE IS A CHILD PROCESS, and it has to be: the running root is read from the process's
/// own directory, which is process-global, and this file's tests are THREADS of one binary — the
/// same reason item 802's environment splits are driven through a seam instead of `set_var`.
///
/// ⚠ It stands the child in `/` rather than in a manufactured second workspace. That drives the
/// `Unknown` refusal instead of `Skewed`, and it is the honest trade: a fabricated tree would need
/// a scratch root of its own — a harness site this crate would then owe the item-795 ratchet — to
/// drive an arm the injected seam above already covers. What only a child can show is that the
/// wiring exists at all, and `/` shows it while leaving nothing behind.
///
/// ⚠⚠⚠ ONE NAMED SIBLING, never the whole binary: a child that ran every test would run THIS one,
/// and a test that spawns itself does not terminate.
///
/// # ⛔⛔⛔⛔⛔ WHICH sibling is the whole arm, and the first choice was a DEAD CONTROL
///
/// Measured 2026-09-01, in this file: the child was first
/// `the_tree_this_suite_judges_holds_this_gates_own_source`, and a mutation that made
/// `workspace_root` ANSWER instead of refusing left this arm **GREEN** — because that sibling
/// panics on its own `let … else` before ever calling the door. It proved *a test refuses*, which
/// nobody doubted, and said nothing about the wiring it exists for.
///
/// ⚠⚠ THE CHILD IS THEREFORE THE ONE WHOSE ONLY ROUTE TO A REFUSAL IS THE DOOR.
/// `no_source_walks_up_out_of_its_crate_without_going_through_the_door` reaches
/// `sprag_gate::sources::rust_sources`, which reaches `workspace_root` and nothing else that can
/// refuse — so a door that answered would let the child PASS, and this arm's `!success` is what
/// catches it. Two ways to produce one red is the collapse this file is about, one level up.
#[test]
fn a_run_that_cannot_name_its_tree_is_refused_by_the_binary_and_not_only_by_the_policy() {
    let me = std::env::current_exe().expect("this test binary's own path");
    let out = std::process::Command::new(&me)
        .args([
            "--exact",
            "no_source_walks_up_out_of_its_crate_without_going_through_the_door",
        ])
        .current_dir("/")
        .output()
        .expect(
            "re-run one of this binary's own tests from a directory with no workspace above it",
        );

    let said = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr),
    );
    assert!(
        !out.status.success(),
        "⛔ ITEM 810: standing in `/`, nothing above declares `[workspace]`, so this binary cannot \
         say which tree it is judging — and it ANSWERED ANYWAY. A root that cannot be established \
         is the state item 809 is about, and a walk nobody can attribute is not a claim: {said}",
    );
    assert!(
        said.contains("REGISTER ITEM 809"),
        "⚠ it failed for some other reason, which proves nothing about the wiring: {said}",
    );
    assert!(
        said.contains("cannot \nbe established") || said.contains("cannot be established"),
        "⚠⚠ AND IT IS THE UNNAMEABLE-ROOT REFUSAL, not the skew one. Two refusals that a caller \
         cannot tell apart are the collapse this whole file is about: {said}",
    );
}

/// ── 4. THE RATCHET: NOBODY SPELLS THE COMPILE-TIME ROOT OUTSIDE THE DOOR ─────────────────────
///
/// ⚠ The needle is the two-level walk specifically, and not the macro alone. A crate finding its
/// OWN fixtures with `env!("CARGO_MANIFEST_DIR")` is asking a different question and is correct —
/// the distinction `a_ratchet_that_reads_the_tree_runs_before_the_commit_lands` already draws in
/// its own vocabulary, for the same reason.
#[test]
fn no_source_walks_up_out_of_its_crate_without_going_through_the_door() {
    let exempt = [OWNS_THE_SPELLING, QUOTES_ITSELF, NAMES_THE_SHAPE];
    let mut offenders = Vec::new();
    let mut seen_exempt = Vec::new();

    for source in rust_sources() {
        let walks: Vec<usize> = source
            .code
            .iter()
            .filter(|(_, line)| line.contains("CARGO_MANIFEST_DIR") && line.contains(".."))
            .map(|(at, _)| *at)
            .collect();
        if walks.is_empty() {
            continue;
        }
        match exempt.iter().find(|(file, _)| *file == source.file) {
            Some((file, _)) => seen_exempt.push((*file).to_owned()),
            None => offenders.push(format!("{}:{:?}", source.file, walks)),
        }
    }

    // ⚠⚠ THE POSITIVE CONTROL COMES FIRST. If the walk cannot find the sites that are SUPPOSED to
    // be there, "no offenders" proves nothing at all — the shape item 799 measured going green on
    // an empty population.
    for (file, why) in exempt {
        assert!(
            seen_exempt.iter().any(|seen| seen == file),
            "⚠⚠ THE SCAN IS BLIND: {file} is exempt because {why}, and the walk did not find the \
             spelling in it. Either the file stopped carrying it — delete the exemption — or this \
             gate is reading the wrong tree and every verdict below is worthless. Found: \
             {seen_exempt:?}",
        );
    }

    assert!(
        offenders.is_empty(),
        "⛔ ITEM 809: these walk up out of their crate with a root baked in at COMPILE time, \
         which is a hole straight past the check `sprag_gate::sources::workspace_root` performs — \
         it compares the compiled-in tree against the one the run is standing in and refuses when \
         they differ. Call it instead: {offenders:?}",
    );
}
