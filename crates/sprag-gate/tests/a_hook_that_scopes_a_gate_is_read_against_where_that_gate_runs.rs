//! `pre-push`'s reason for SCOPING the pixel smoke is read against CI, which also runs it —
//! register item 611, and the misdiagnosis it caused in item 608.
//!
//! # What went wrong without this
//!
//! The hook's own prose said the smoke *"runs in neither CI ... nor pre-commit"*, and that sentence
//! is what justifies `PIXEL_PATHS` being narrow: if nothing else ever runs the smoke, a push that
//! does not touch those paths is the one place it could have been caught. CI grew a `pixel-linux`
//! job that runs the very same binary under xvfb on EVERY push, and the sentence stayed.
//!
//! On 2026-08-22 a round read that sentence, concluded the shipped topology passed through no gate
//! at all, and filed it — while `pixel-linux` had been green on the same commit the whole time. The
//! same round had already used the same sentence to explain a local-only failure as *"red for a
//! while and nobody ran it"*. **One stale line of prose produced two wrong diagnoses in one round**,
//! which is item 416's shape: a document that asserts a state ages, and nothing tells anyone.
//!
//! # Why this shape and not a spell-check
//!
//! Item 470's rule: a ratchet that cannot rot reads BOTH artefacts. Matching on the stale wording
//! would only outlaw one phrasing, and the next rewrite escapes it. So the requirement is positive
//! and stated in CI's own terms — **if CI runs the smoke, the hook that scopes it must NAME the job
//! that does** — and it fires in both directions:
//!
//! * CI runs the smoke and the hook does not name the job: the hook's scoping argument is being
//!   made against a world that no longer exists. Red.
//! * The hook names a job CI does not have: the reference is dead, which is the same rot pointing
//!   the other way. Red.
//!
//! Renaming the CI job therefore forces the hook's prose to follow, which is exactly the coupling
//! that was missing.

use sprag_gate::sources::workspace_root;

/// The binary whose runs are being reconciled. Both artefacts spell it the same way.
const SMOKE: &str = "sprag-smoke";

/// How CI actually RUNS it, which is the only occurrence that means CI runs it.
///
/// ⚠ The sibling of [`declaration`]'s lesson, and it was carrying the same defect: the workflow
/// names `sprag-smoke` twice in prose explaining why the job exists, so a bare containment check
/// would have gone on passing after the run line was deleted.
const INVOCATION: &str = "./target/release/sprag-smoke";

/// The CI job that runs it, as CI names it.
///
/// ⚠ The job's `name:`, not the yaml key: the name is what a person reads in the checks list and so
/// what the hook's prose should be pointing them at.
const JOB: &str = "pixel (linux)";

/// The same name as CI DECLARES it, which is the only occurrence that decides anything.
///
/// ⚠⚠⚠⚠⚠ The first cut of this gate asked whether the workflow merely CONTAINED the name, and the
/// mutation that renamed the job came back GREEN: the workflow also mentions the job in a comment,
/// so the check was reading prose about the job instead of the job. A substring found anywhere is
/// not a declaration — ask the line that decides.
fn declaration() -> String {
    format!("name: {JOB}")
}

/// The hook that scopes the smoke, and therefore owes the reader the fact that CI does not.
const HOOK: &str = ".githooks/pre-push";

/// CI's workflow.
const CI: &str = ".github/workflows/ci.yml";

fn read(relative: &str) -> String {
    let path = workspace_root().join(relative);
    std::fs::read_to_string(&path)
        .unwrap_or_else(|why| panic!("{} is unreadable: {why}", path.display()))
}

#[test]
fn a_hook_that_scopes_the_pixel_smoke_names_the_ci_job_that_runs_it_anyway() {
    let ci = read(CI);
    let hook = read(HOOK);

    // The premise, asserted rather than assumed — this gate is about a DISAGREEMENT, and with no
    // CI job to disagree with it would pass for the wrong reason.
    assert!(
        ci.contains(INVOCATION),
        "{CI} does not RUN {SMOKE} at all, so this gate is measuring nothing; if the job was \
         removed on purpose, this test is what should be deleted with it",
    );
    assert!(
        ci.contains(&declaration()),
        "{CI} no longer DECLARES a job named {JOB:?}, so the name this gate holds the hook to is \
         dead; update both, not just CI",
    );

    assert!(
        hook.contains(JOB),
        "{HOOK} scopes the pixel smoke with `PIXEL_PATHS`, and the argument for scoping it rests on \
         where else the smoke runs — but {CI} runs it on every push as {JOB:?} and the hook does \
         not say so. A reader of the hook concludes the smoke runs nowhere else, which is what \
         items 608 and 611 both got wrong. Name the job in the hook's comment.",
    );
}
