//! The quiescence gate's proof, as a count — because it no longer has one as behaviour.
//!
//! ## What was lost, and why a count is what replaces it
//!
//! [`Tracker`]'s skip is EXACT: it claims a re-evaluation of a pane where nothing the rules read
//! has moved would reach the answer already published. An exact skip is by definition one whose
//! absence changes no answer, so behaviour cannot see it — and R252's instrument worked only
//! because one input to the rules was, at the time, outside the gate's key. It rewrote a rule
//! underneath a settled tracker and asserted the verdict did NOT move, which only an evaluation
//! could have contradicted. R254 put the rule list's identity INTO the key, which is the correct
//! fix for a real defect and which consumed that instrument: a rewritten list is now a different
//! list, the gate no longer skips it, and deleting the gate outright turned no test red.
//!
//! So the observable moves from the answer to the WORK, and this project already knows which form
//! of work is assertable. R221 measured wall-clock on this box drifting 20-30% between runs of the
//! same binary, so a microsecond threshold would be a flake by construction; R217 metered the
//! projection path in counts for the same reason. `sprag-latency` says what the saving is worth in
//! time. These tests are the half that goes red.
//!
//! ## Why this is an integration test and not a unit one
//!
//! [`sprag_detect::work`] reads PROCESS-WIDE counters, so a delta is only sound when nothing else
//! in the process is evaluating at the same time. This crate's unit module runs dozens of
//! evaluations on the default parallel harness, so a delta taken there would be measuring whichever
//! tests happened to be in flight — green on a quiet machine, red on a busy one, and meaningless
//! either way. A test binary of its own, whose only evaluations are these, is what makes an exact
//! number assertable; the mutex serialises the tests inside it.

use std::sync::Mutex;
use std::time::{Duration, Instant};

use sprag_detect::{
    AgentState, DEFAULT_SETTLE, Hysteresis, Report, Ruleset, Tracker, built_ins, detect, work,
};
use sprag_vt::{Emulator, VtPort};

/// Serialises the tests in this binary, so each one's delta is its own.
static SERIAL: Mutex<()> = Mutex::new(());

/// A `claude` pane the footer fingerprint claims — the first manifest in the built-in list, so a
/// look at it stops after one.
const CLAUDE_FOOTER: &[&str] = &["❯", "  ⏸ manual mode on · ? for shortcuts"];

/// An ordinary shell pane: nothing claims it, so every manifest is offered one.
const SHELL: &[&str] = &["$ ls -la", "total 0", "$ "];

/// A pane showing a yes/no dialog — blocked on sight, whoever claims it.
const DIALOG: &[&str] = &["❯ 1. Yes", "  2. No"];

fn painted(lines: &[&str]) -> Emulator {
    let mut em = Emulator::new(80, 24);
    em.advance(lines.join("\r\n").as_bytes());
    em
}

fn repaint(em: &mut Emulator, lines: &[&str]) {
    em.advance(b"\x1b[2J\x1b[H");
    em.advance(lines.join("\r\n").as_bytes());
}

/// THE GATE, as the number it holds down.
///
/// A settled pane observed again and again — which is what every client wake does to every pane in
/// the workspace — must run the rules ONCE. Delete the skip in [`Tracker::observe`] and this reads
/// the number of looks instead.
#[test]
fn a_pane_that_has_not_moved_is_evaluated_once_however_often_it_is_observed() {
    let _serial = SERIAL
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let rules = Ruleset::default();
    let mut tracker = Tracker::new(Hysteresis::default());
    let em = painted(CLAUDE_FOOTER);
    let title = Some("✳ Claude Code");
    // Settled first, and OUTSIDE the measured window: a first sighting evaluates, and a candidate
    // resting on an absence needs its window to close before the pane is at rest at all.
    let base = Instant::now();
    tracker.observe(em.screen(), title, &rules, base);
    tracker.observe(em.screen(), title, &rules, base + DEFAULT_SETTLE);

    let before = work();
    for look in 0..64 {
        tracker.observe(
            em.screen(),
            title,
            &rules,
            base + DEFAULT_SETTLE * (2 + look),
        );
    }
    let after = work();

    assert_eq!(
        after.evaluations_total - before.evaluations_total,
        0,
        "64 looks at a pane that has not painted a row cost no evaluation at all",
    );
    assert_eq!(
        after.manifests_total - before.manifests_total,
        0,
        "and no manifest is offered a screen it has already been shown",
    );
}

/// The gate's OTHER half, and the one that says the skip is a skip rather than a freeze: a pane
/// that paints is evaluated again, exactly once per look.
#[test]
fn a_pane_that_moves_is_evaluated_once_per_look() {
    let _serial = SERIAL
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let rules = Ruleset::default();
    let mut tracker = Tracker::new(Hysteresis::default());
    let mut em = painted(CLAUDE_FOOTER);
    let title = Some("✳ Claude Code");
    let base = Instant::now();
    tracker.observe(em.screen(), title, &rules, base);

    let before = work();
    for look in 0..8 {
        // A repaint moves the row generations, which is the input the gate watches.
        repaint(&mut em, CLAUDE_FOOTER);
        tracker.observe(em.screen(), title, &rules, base + DEFAULT_SETTLE * look);
    }
    let after = work();

    assert_eq!(
        after.evaluations_total - before.evaluations_total,
        8,
        "eight repaints, eight evaluations — the gate skips what has not changed and nothing else",
    );
}

/// R254's third input, metered: a reload is not visible on any pane's screen, so the count is the
/// only place a look at a settled pane after an edit can show up.
#[test]
fn a_reload_costs_every_remembered_pane_exactly_one_evaluation() {
    let _serial = SERIAL
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let mut tracker = Tracker::new(Hysteresis::default());
    let em = painted(CLAUDE_FOOTER);
    let title = Some("✳ Claude Code");
    let base = Instant::now();

    let rules = Ruleset::default();
    tracker.observe(em.screen(), title, &rules, base);
    tracker.observe(em.screen(), title, &rules, base + DEFAULT_SETTLE);

    // The user edits `config.toml`: a NEW list, with the same rules in it. Nothing on the screen
    // moved, and the answer will not move either — the evaluation is what proves the edit arrived.
    let edited = Ruleset::new(built_ins());

    let before = work();
    tracker.observe(em.screen(), title, &edited, base + DEFAULT_SETTLE * 2);
    tracker.observe(em.screen(), title, &edited, base + DEFAULT_SETTLE * 3);
    let after = work();

    assert_eq!(
        after.evaluations_total - before.evaluations_total,
        1,
        "the first look after a reload evaluates; the second is quiet again",
    );
}

/// R217's lesson, one crate over: the one way a meter can lie is by counting a delegation twice.
/// [`Tracker`] identifies through [`detect`] rather than walking the list itself, so a look that
/// evaluates must be ONE evaluation and not two.
#[test]
fn the_trackers_delegation_to_detect_is_not_metered_twice() {
    let _serial = SERIAL
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let rules = Ruleset::default();
    let em = painted(CLAUDE_FOOTER);
    let title = Some("✳ Claude Code");

    let before = work();
    let _ = detect(em.screen(), title, rules.manifests());
    let direct = work();
    Tracker::new(Hysteresis::default()).observe(em.screen(), title, &rules, Instant::now());
    let through_tracker = work();

    assert_eq!(
        direct.evaluations_total - before.evaluations_total,
        1,
        "one call to the matcher is one evaluation",
    );
    assert_eq!(
        through_tracker.evaluations_total - direct.evaluations_total,
        1,
        "and so is one look through the tracker that delegates to it",
    );
}

/// The VOLUME, and the asymmetry slice 4's layering rule charges to every pane that is not an
/// agent: identification stops at the first manifest that claims the pane, so a claimed pane costs
/// its position in the list and an ordinary shell pane costs the whole list.
#[test]
fn a_pane_nobody_claims_is_offered_every_manifest_and_a_claimed_one_stops() {
    let _serial = SERIAL
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let rules = Ruleset::default();
    let manifests = rules.manifests();
    assert!(
        manifests.len() > 1,
        "the built-in list must have more than one entry for this to distinguish anything",
    );

    let shell = painted(SHELL);
    let before = work();
    let _ = detect(shell.screen(), None, manifests);
    let after_shell = work();
    assert_eq!(
        after_shell.manifests_total - before.manifests_total,
        manifests.len() as u64,
        "nothing claims a shell, so every manifest is asked",
    );

    let claude = painted(CLAUDE_FOOTER);
    let _ = detect(claude.screen(), Some("✳ Claude Code"), manifests);
    let after_claude = work();
    assert_eq!(
        after_claude.manifests_total - after_shell.manifests_total,
        1,
        "the first manifest claims it, and the rest are never consulted",
    );
}

/// A reported pane does not run the rules at all — the cheapness claim, measured rather than argued.
///
/// # Why it lives HERE and not beside the tracker it is about
///
/// It was written in `track.rs`'s unit module, where this file's own preamble says a meter delta
/// cannot be taken: dozens of neighbouring tests evaluate on the default parallel harness, so the
/// delta measures whichever of them was in flight. It passed for rounds and then failed in a full
/// workspace run at R338 — 86 against 85, one neighbour's evaluation landing between the two reads —
/// which is the failure the rule three paragraphs up predicted, in the crate that wrote the rule.
///
/// Overruling the screen AFTER evaluating it would be correct and would pay for every pattern of
/// every manifest on a path served per client wake, so the number is the claim.
#[test]
fn a_reported_pane_costs_no_evaluation() {
    let _serial = SERIAL
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let rules = Ruleset::new(vec![sprag_detect::claude(), sprag_detect::codex()]);
    let mut tracker = Tracker::default();
    let mut em = painted(DIALOG);
    let base = Instant::now();

    tracker.observe(em.screen(), Some("claude"), &rules, base);
    tracker.report(Report {
        state: AgentState::Working,
        agent: None,
        source: "hook".to_owned(),
        seq: None,
        owner: None,
    });

    // The screen moves, so the quiescence gate is not what is being measured here.
    repaint(&mut em, CLAUDE_FOOTER);
    let before = work().evaluations_total;
    tracker.observe(
        em.screen(),
        Some("claude"),
        &rules,
        base + Duration::from_secs(1),
    );
    assert_eq!(
        work().evaluations_total,
        before,
        "a reported pane's screen is recorded, not evaluated",
    );

    // The control: released, the same moved screen DOES cost an evaluation.
    tracker.release_report();
    repaint(&mut em, DIALOG);
    let before = work().evaluations_total;
    tracker.observe(
        em.screen(),
        Some("claude"),
        &rules,
        base + Duration::from_secs(2),
    );
    assert!(
        work().evaluations_total > before,
        "and the scrape still runs when it is the authority",
    );
}
