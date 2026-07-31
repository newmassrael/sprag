//! The pane list does not walk the agent registry, and this is what says so.
//!
//! ## What this gates, and why nothing else could
//!
//! `AgentClock::observe` used to read the registry's NEAREST DEADLINE before and after every look,
//! to decide whether the settle waker needed telling. It is the correct question and it visits every
//! tracker to answer it — so the pane list, which calls `observe` once per pane, performed 2N^2
//! tracker visits per client wake. R255 measured the term at 2.70 to 3.35 ns per remembered pane per
//! look, linear in the entry count against a control that ruled out cache locality, and R256 asked
//! the O(1) question instead: only the observed pane's tracker can have changed, so only its
//! deadline can have moved the minimum.
//!
//! That change moves no verdict, wakes no different client and alters no wire byte. It is exactly
//! the class R255 recorded as needing a count rather than a behaviour: an exact optimisation has no
//! behavioural observable, so without this the tidier-looking whole-registry read comes back the
//! first time somebody simplifies the function, and nothing goes red. `sprag_grid::work` and
//! `sprag_detect::work` are the same instrument for the same reason.
//!
//! ## Why an integration test
//!
//! `sprag_host::agent::work` reads a PROCESS-WIDE counter, and this crate's unit tests run in
//! parallel — several of them park wakers, which is a legitimate scan. A delta taken there would be
//! measuring whichever test happened to be parking. A test binary whose only scans are these is what
//! makes an exact number assertable; the mutex serialises the two tests inside it.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use sprag_detect::Hysteresis;
use sprag_host::{AgentClock, agent};
use sprag_terminal::PaneId;
use sprag_vt::{Emulator, VtPort};

/// Serialises the tests in this binary, so each one's delta is its own.
static SERIAL: Mutex<()> = Mutex::new(());

/// A `claude` pane the footer fingerprint claims, so every tracker below holds a real verdict
/// rather than an empty one.
const CLAUDE_FOOTER: &[&str] = &["❯", "  ⏸ manual mode on · ? for shortcuts"];
const CLAUDE_TITLE: &str = "✳ Claude Code";

/// Far more panes than any pane list this project measures, so a per-entry cost cannot hide.
const REMEMBERED: u64 = 64;

fn painted() -> Emulator {
    let mut em = Emulator::new(80, 24);
    em.advance(CLAUDE_FOOTER.join("\r\n").as_bytes());
    em
}

/// A clock remembering [`REMEMBERED`] settled panes.
fn crowded(em: &Emulator, base: Instant) -> AgentClock {
    let clock = AgentClock::default();
    for id in 0..REMEMBERED {
        // Twice, so every tracker is SETTLED rather than pending: a registry full of waiting
        // candidates is a different measurement from the quiet workspace this is about.
        for at in [base, base + sprag_detect::DEFAULT_SETTLE] {
            clock.observe(
                PaneId(id),
                em.screen(),
                Some(CLAUDE_TITLE),
                at,
                Hysteresis::default,
            );
        }
    }
    clock
}

/// THE GATE: a look at one pane costs nothing per OTHER pane the registry remembers.
///
/// Restore the whole-registry read in `AgentClock::observe` and this reads 128 — two scans of
/// sixty-four trackers — instead of nothing.
#[test]
fn a_look_at_one_pane_does_not_walk_the_registry() {
    let _serial = SERIAL
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let em = painted();
    let base = Instant::now();
    let clock = crowded(&em, base);
    let settled = base + sprag_detect::DEFAULT_SETTLE;

    let before = agent::work();
    for _ in 0..8 {
        clock.observe(
            PaneId(0),
            em.screen(),
            Some(CLAUDE_TITLE),
            settled,
            Hysteresis::default,
        );
    }
    let after = agent::work();

    assert_eq!(
        after.deadline_visits_total - before.deadline_visits_total,
        0,
        "eight looks at one pane visited another pane's tracker, so the pane list is quadratic \
         again in the number of panes a session has open",
    );
}

/// The other half, so the counter is not passing by counting nothing: the scan still EXISTS where it
/// belongs, and the waker pays it once per park rather than once per pane.
///
/// Without this a meter reading zero would be indistinguishable from a meter that was never wired
/// up, which is the way this kind of gate rots.
#[test]
fn the_wakers_own_park_still_reads_the_whole_registry_once() {
    let _serial = SERIAL
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let em = painted();
    let base = Instant::now();
    let clock = Arc::new(crowded(&em, base));

    let before = agent::work();
    // Nothing is pending, so this parks for the cap and returns; the cap is short because the
    // subject is the READ the park performs, not the sleep.
    clock.park_until_due(Duration::from_millis(20));
    let after = agent::work();

    assert_eq!(
        after.deadline_visits_total - before.deadline_visits_total,
        REMEMBERED,
        "one park asks the registry as a whole exactly once, over every tracker in it",
    );
}
