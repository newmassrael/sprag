//! The settle waker's SWEEP: one pass over every pane the daemon has, deciding which of them owe an
//! evaluation and paying it for those that do.
//!
//! # Why this is a module and not a closure in the daemon
//!
//! It was a closure in `sprag-term`'s waker thread through slices 3 and 4. Every input it reads is
//! a library type — the [`SessionRegistry`], the [`AgentClock`], the [`ChannelRegistry`] — so the
//! binary held host logic that the library owned every piece of, and the shape of that mistake was
//! the usual one: **the pass could not be called, so it could not be tested and could not be
//! measured.** R260 priced its TERMS one at a time (the per-pane question, the whole-registry read,
//! the prune, the census, the manifest re-read) and had to leave the composition unpriced, because
//! summing terms is not the same claim as running the thing. R261 needed the real pass running
//! against a real registry while another thread served requests, and that is what forced the split.
//!
//! What stays in the daemon is the SCHEDULING — the thread, the park, the `last_sweep` clock, and
//! the manifest re-read that must happen before a pass so the panes an edit invalidates are served
//! by the very pass that invalidated them. What lives here is the WORK, which is a pure function of
//! the registry, the clock and two arguments.
//!
//! # What it costs, and the part that is not the obvious part
//!
//! Measured R261 on a live registry with real PTY panes, `--release`, five runs:
//!
//! * **A quiet pass is free to everybody else.** Every pane settled under unchanged rules, the whole
//!   pass 0.37 to 0.58 us, and a concurrent pane-list reader cannot tell whether it is running:
//!   +0.4 to +0.8 us on its median against a control doing the same work on a private registry, at
//!   SEVEN TO TWELVE MILLION times the real duty cycle.
//! * **A pass in which every pane OWES an evaluation is a different object** — 44 to 58 us for three
//!   panes, about 100x the quiet one, because the workspace lock is held across
//!   [`AgentClock::observe`] for every pane in that window. That is not hypothetical: **a manifest
//!   reload makes every remembered pane stale at once**, so saving `config.toml` schedules exactly
//!   one such pass, and so does the first pass after boot.
//! * **The reader pays a bigger share of a reload than the lock does.** A pane-list request runs the
//!   same detector under the same clock, so after a reload the reader evaluates too — 45 to 62 us on
//!   its own median with no second thread in the process at all.
//!
//! The numbers, the conditions and the one comparison that cannot be made are on [`sweep_once`].

use std::collections::{HashMap, HashSet};
use std::sync::{Mutex, PoisonError};
use std::time::Instant;

use sprag_terminal::{PaneId, SessionRegistry, Workspace};

use crate::agent::AgentClock;
use crate::events::Event;
use crate::notify::ChannelRegistry;

/// What one pass did — returned so a caller can act on it and a test can assert it, rather than
/// inferring the pass's behaviour from its side effects.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct SweepReport {
    /// Panes visited, which is every pane the daemon has.
    pub visited: usize,
    /// Panes whose screen was actually read — the ones that owed an evaluation. Zero is the steady
    /// state and the number this pass's cost is dominated by when it is not.
    pub evaluated: usize,
    /// Sessions whose published answer MOVED, and so whose clients were woken. A subset of the
    /// sessions visited, and empty on a pass that changed nothing.
    pub moved: usize,
}

/// One pass of the sweep: visit every pane, evaluate the ones that owe it, prune the trackers of
/// panes that are gone, and wake the clients of the sessions whose answer moved.
///
/// `now` is passed rather than read so a caller can drive the pass at a chosen instant — the tests
/// depend on it and so does any harness that wants a deterministic pass. `discover` is the
/// difference between the waker's two kinds of wake: `false` serves only panes whose deadline has
/// passed, `true` additionally looks for panes nobody has asked about, panes whose ruleset has been
/// replaced, and trackers whose pane is gone. The daemon passes `true` once per
/// [`crate::agent::SWEEP_INTERVAL`].
///
/// # Lock discipline
///
/// The registry lock is taken and RELEASED to clone out the pools, then each workspace lock is taken
/// on its own — never nested, which is what keeps this off the dispatch path's back. The clock's
/// lock is taken INSIDE a workspace lock, because the screen is only reachable there, and never the
/// other way round. The pane-list query takes the same two in the same order for the same reason, so
/// the ordering is a property of both callers rather than a convention this one follows.
///
/// # What the locks cost — MEASURED (R261)
///
/// R260 left this as the one term it would not claim, on the grounds that a single-threaded
/// instrument measures a lock uncontended and uncontended is not what a lock costs. What it costs is
/// the WAIT it inflicts, so the subject is a READER's latency while this runs — measured against a
/// thread serving pane-list requests, with the pass running CONTINUOUSLY rather than once every five
/// seconds, so that a negligible answer settles the real cadence a fortiori.
///
/// **The recurring pass is free.** Shared minus a control sweeping a PRIVATE registry at the same
/// rate: +0.4 to +0.8 us on the reader's median and -2.4 to +0.9 us at p99 — at seven to twelve
/// million times the daemon's actual duty cycle. Taken again while another project was building
/// (the tool's control spread 29-240% against 25-69%), the same differences read -1.4 to +5.9 and
/// -1.6 to +6.4: the paired design holds across a 2x change in the box, which is what it is for.
///
/// **The churning pass cannot be answered the same way, and that is a property of the system rather
/// than of the instrument.** A pane-list request runs the same detector under the same clock, so
/// after a reload the reader evaluates the panes too, and whichever of the two threads arrives first
/// pays. Sharing the registry therefore changes WHO evaluates and not only who waits: the private
/// control's sweeper evaluated three panes on every pass while the shared one evaluated a fraction,
/// the reader having done the rest. The two conditions cannot be matched, so their difference is not
/// a lock cost — R255's shape again, a comparison that cannot be resolved at the level it was asked.
///
/// So that case is bounded DIRECTLY instead, by this pass's own duration: 44 to 58 us for three
/// panes against 0.37 to 0.58 us quiet, about 100x, and a reader wants one window so what it can
/// wait for is that window's share. The consequence is stated rather than left to be met: a manifest
/// edit schedules one pass in which every remembered pane is stale. It is a one-off on a user action,
/// which is why this is documented and not redesigned — and it is the paragraph a future slice that
/// makes evaluations dearer, or lets one window hold many more panes, has to come back and re-read.
pub fn sweep_once(
    registry: &Mutex<SessionRegistry>,
    agents: &AgentClock,
    channels: &ChannelRegistry,
    now: Instant,
    discover: bool,
) -> SweepReport {
    // Phase 1 — registry lock ONLY: clone out each session's pools as handles, keeping the session
    // NAME beside each so a published change can wake that session's clients and no others.
    let pools: Vec<(String, std::sync::Arc<Mutex<Workspace>>)> = {
        let reg = registry.lock().unwrap_or_else(PoisonError::into_inner);
        reg.sessions()
            .iter()
            .flat_map(|session| {
                let name = session.name().to_owned();
                session
                    .windows()
                    .iter()
                    .map(move |window| (name.clone(), std::sync::Arc::clone(window.workspace())))
            })
            .collect()
    };

    // Phase 2 — registry lock released. Each pool under its own lock.
    let mut report = SweepReport::default();
    let mut live: HashSet<PaneId> = HashSet::new();
    // Per SESSION, the panes whose published verdict moved — the wake and its reason, collected
    // together so they cannot disagree. `seq` moving is the sweep's own definition of a published
    // change and is now also the record's, rather than a second rule that could drift from it.
    let mut moved: HashMap<String, Vec<u64>> = HashMap::new();
    for (session, pool) in &pools {
        let pool = pool.lock().unwrap_or_else(PoisonError::into_inner);
        for pane in pool.panes() {
            let id = pane.id();
            live.insert(id);
            report.visited += 1;
            // Three reasons to ask about a pane — due, unknown, stale — and none of them applies to
            // a settled pane under unchanged rules, which is every pane in a quiet workspace.
            // `AgentRegistry::owes_evaluation` holds the composition and the argument for each.
            if !agents.with(|state| state.owes_evaluation(id, now, discover)) {
                continue;
            }
            report.evaluated += 1;
            let before = agents.with(|state| state.seq(id));
            let title = pane.title();
            pane.pty().with_screen(|screen| {
                agents.observe(
                    id,
                    screen,
                    title.as_deref(),
                    now,
                    crate::config::agent_settle,
                );
            });
            if agents.with(|state| state.seq(id)) != before {
                moved.entry(session.clone()).or_default().push(id.0);
            }
            // THE LOOP'S LIVENESS RESTS ON THIS. `park_until_due` returns immediately for a deadline
            // already past, so a due pane that an observation does not RESOLVE sends the waker round
            // at full speed forever. It cannot happen: `is_due` is `since + settle <= now`, which is
            // exactly `settle`'s own publish condition, and every other path through `observe` either
            // publishes or re-dates the candidate to `now`. Stated as an assertion rather than a
            // comment because the failure is a spin rather than a wrong answer — the mutation that
            // removed the observe above took the scene revision from 264 to 6,178,283 in twelve
            // seconds.
            debug_assert!(
                !agents.with(|state| state.is_due(id, now)),
                "pane {id} is still due after being observed at the same instant",
            );
        }
    }

    // A tracker must not outlive its pane. The census is DAEMON-WIDE, which is why it is built here
    // and never in the pane-list query: that walk sees one session, and pruning against it would
    // forget every other session's panes.
    if discover {
        agents.with(|state| state.retain_live(&live));
    }
    // Wake the clients of the sessions whose published answer moved, and only those — and tell
    // them WHICH panes moved, in the same call, so the wake and its reason land together
    // (`ChannelRegistry::announce` holds the journal lock across the bump for exactly that).
    for (session, panes) in &moved {
        channels.announce(
            session,
            panes
                .iter()
                .copied()
                .map(Event::AgentStateChanged)
                .collect(),
        );
    }
    report.moved = moved.len();
    report
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    use sprag_detect::{DEFAULT_SETTLE, Ruleset, built_ins};
    use sprag_terminal::CommandBuilder;

    /// A pane that paints `text` and then blocks on its PTY forever, so nothing it does can land in
    /// the middle of a pass.
    fn painting(text: &str) -> CommandBuilder {
        let mut command = CommandBuilder::new("/bin/sh");
        command.arg("-c");
        command.arg(format!("printf '%s' \"{text}\"; exec cat"));
        command.env("TERM", "dumb");
        command
    }

    /// The `claude` footer fingerprint — a pane the first built-in manifest claims, so its verdict
    /// can MOVE. A plain shell pane's never does, which is what the two-session test needs.
    fn claude_pane() -> CommandBuilder {
        painting("  \u{23f8} manual mode on \u{b7} ? for shortcuts")
    }

    /// A registry with one session per entry in `panes`, each holding that one pane, and every pane
    /// waited for so a pass reads a painted screen rather than a blank one.
    fn registry_with(panes: &[(&str, CommandBuilder)]) -> Arc<Mutex<SessionRegistry>> {
        let reg = Arc::new(Mutex::new(SessionRegistry::new((80, 24))));
        for (session, command) in panes {
            let pool = {
                let mut guard = reg.lock().unwrap_or_else(PoisonError::into_inner);
                guard
                    .new_session(Some(session))
                    .expect("a fresh session name");
                guard.workspace_of(session).expect("the session just made")
            };
            let mut guard = pool.lock().unwrap_or_else(PoisonError::into_inner);
            guard
                .spawn(command.clone(), (*session).to_owned(), 80, 24)
                .expect("a pane on a pty");
        }
        wait_for_paint(&reg);
        reg
    }

    /// Block until every pane has painted something, reading the panes rather than sleeping for a
    /// guessed interval — a pass over a blank screen is a different measurement.
    fn wait_for_paint(reg: &Arc<Mutex<SessionRegistry>>) {
        let deadline = Instant::now() + std::time::Duration::from_secs(10);
        while Instant::now() < deadline {
            let pools: Vec<_> = {
                let guard = reg.lock().unwrap_or_else(PoisonError::into_inner);
                guard
                    .sessions()
                    .iter()
                    .flat_map(|s| s.windows().iter().map(|w| Arc::clone(w.workspace())))
                    .collect()
            };
            let painted = pools.iter().all(|pool| {
                let guard = pool.lock().unwrap_or_else(PoisonError::into_inner);
                guard.panes().iter().all(|pane| {
                    pane.pty().with_screen(|screen| {
                        screen.row_generation(0).is_some_and(|written| written > 0)
                    })
                })
            });
            if painted {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        panic!("a pane never painted");
    }

    fn clock() -> AgentClock {
        AgentClock::new(Ruleset::default())
    }

    /// The pass's whole cost story, at the level the cost is claimed: the FIRST pass over a daemon
    /// evaluates every pane because nobody has ever looked at one, and the next pass over the same
    /// quiet workspace evaluates NONE.
    ///
    /// R253 found the first half by driving the binary — until slice 3 the only thing that observed
    /// was the pane-list query, so a daemon nobody had queried held no state for any pane. R260
    /// priced the second half. Neither could be asserted at this level before the pass was a
    /// function.
    #[test]
    fn a_first_pass_discovers_every_pane_and_the_next_evaluates_none() {
        let reg = registry_with(&[("a", claude_pane()), ("b", painting("$ "))]);
        let agents = clock();
        let channels = ChannelRegistry::default();
        let base = Instant::now();

        let first = sweep_once(&reg, &agents, &channels, base, true);
        assert_eq!(first.visited, 2);
        assert_eq!(
            first.evaluated, 2,
            "a pane nobody has looked at is invisible to every other question, so only a sweep \
             can give it a state",
        );

        let second = sweep_once(&reg, &agents, &channels, base, true);
        assert_eq!(second.visited, 2);
        assert_eq!(
            second.evaluated, 0,
            "the steady state of a quiet workspace, and the answer the sweep's cost rests on",
        );
    }

    /// A reload makes the next pass evaluate every pane again — slice 4's layering, asserted where
    /// the work actually happens rather than on the registry alone.
    #[test]
    fn a_reload_makes_the_next_pass_evaluate_every_pane() {
        let reg = registry_with(&[("a", claude_pane())]);
        let agents = clock();
        let channels = ChannelRegistry::default();
        let base = Instant::now();

        sweep_once(&reg, &agents, &channels, base, true);
        assert_eq!(
            sweep_once(&reg, &agents, &channels, base, true).evaluated,
            0
        );

        agents.with(|state| state.reload(Ruleset::new(built_ins())));
        assert_eq!(
            sweep_once(&reg, &agents, &channels, base, true).evaluated,
            1,
            "the input that moved was not on the pane's screen, so nothing else will bring it back",
        );
    }

    /// A wake that is not a SWEEP serves only what is late. Discovery and staleness wait for the
    /// interval, which is the whole meaning of the flag and is decided here rather than by the
    /// caller.
    #[test]
    fn a_wake_that_is_not_a_sweep_discovers_nothing() {
        let reg = registry_with(&[("a", claude_pane())]);
        let agents = clock();
        let channels = ChannelRegistry::default();
        let base = Instant::now();

        let due_only = sweep_once(&reg, &agents, &channels, base, false);
        assert_eq!(due_only.visited, 1, "it still walks — it just does not ask");
        assert_eq!(
            due_only.evaluated, 0,
            "an undiscovered pane is not LATE, and a deadline wake is for what is late",
        );
    }

    /// A tracker must not outlive its pane, and the census that decides it is DAEMON-WIDE.
    ///
    /// The arc register listed this as deliberately uncovered, on the grounds that a leaked tracker
    /// is unobservable through any surface — true while the pass was a closure in a binary. The
    /// registry's own length is the observable, and reaching it only needed the pass to be callable.
    #[test]
    fn a_pass_forgets_the_tracker_of_a_pane_that_is_gone() {
        let reg = registry_with(&[("a", claude_pane()), ("b", painting("$ "))]);
        let agents = clock();
        let channels = ChannelRegistry::default();
        let base = Instant::now();

        sweep_once(&reg, &agents, &channels, base, true);
        assert_eq!(agents.with(|state| state.len()), 2);

        // Session `b`'s pane closes. Its tracker is now the only thing that remembers it.
        {
            let pool = {
                let guard = reg.lock().unwrap_or_else(PoisonError::into_inner);
                guard.workspace_of("b").expect("session b")
            };
            let mut guard = pool.lock().unwrap_or_else(PoisonError::into_inner);
            let gone = guard.panes()[0].id();
            let _closed = guard.close(gone);
        }

        sweep_once(&reg, &agents, &channels, base, true);
        assert_eq!(
            agents.with(|state| state.len()),
            1,
            "a census from one session would have forgotten the other session's panes too",
        );
    }

    /// Only the sessions whose published answer MOVED are woken, and the other one is not.
    ///
    /// The arc register's other deliberately-uncovered property, for the reason it gave — it needs a
    /// two-session harness, which needs a callable pass. The cost of getting it wrong is a redundant
    /// client re-read, which is cheap; the cost of never testing it is that the selectivity is
    /// decoration.
    #[test]
    fn only_the_session_whose_answer_moved_is_woken() {
        let reg = registry_with(&[("a", claude_pane()), ("b", painting("$ "))]);
        let agents = clock();
        let channels = ChannelRegistry::default();
        let base = Instant::now();

        // The first pass gives the claude pane a pending candidate; nothing is published yet,
        // because a verdict resting on an ABSENCE has to hold for the settle window.
        let first = sweep_once(&reg, &agents, &channels, base, true);
        assert_eq!(first.moved, 0, "a candidate is not a publication");
        let (before_a, before_b) = (
            channels.revision("a").current(),
            channels.revision("b").current(),
        );

        // The window closes. Only `a` has anything to say.
        let settled = sweep_once(&reg, &agents, &channels, base + DEFAULT_SETTLE, true);
        assert_eq!(settled.moved, 1);
        assert!(
            channels.revision("a").current() > before_a,
            "the session whose pane published is woken",
        );
        assert_eq!(
            channels.revision("b").current(),
            before_b,
            "and the one whose answer did not move is not — a shell pane no manifest claims \
             publishes nothing, so its clients have nothing to re-read",
        );
    }

    /// **THE slice-4 claim.** The verdict transition is the event the whole niche is about, and it
    /// is the one thing the dispatch funnel structurally cannot derive: it rests on the pane's
    /// SCREEN, which reaches the daemon through output, and on a clock nothing else in the daemon
    /// runs. So the observer that can see it emits it — and the record must name the very pane whose
    /// `seq` moved, which is the same condition the wake beside it already uses.
    #[test]
    fn the_settle_wakes_a_session_and_says_which_pane_moved() {
        let reg = registry_with(&[("a", claude_pane()), ("b", painting("$ "))]);
        let agents = clock();
        let channels = ChannelRegistry::default();
        let base = Instant::now();

        sweep_once(&reg, &agents, &channels, base, true);
        let cursor_a = channels.revision("a").current();
        let cursor_b = channels.revision("b").current();
        assert!(
            journal_events(&channels, "a", 0).is_empty(),
            "a candidate is not a publication, so it is not a record either",
        );

        sweep_once(&reg, &agents, &channels, base + DEFAULT_SETTLE, true);

        let recorded = journal_events(&channels, "a", cursor_a);
        assert_eq!(
            recorded.len(),
            1,
            "exactly the pane whose verdict moved: {recorded:?}",
        );
        assert!(
            matches!(recorded[0], Event::AgentStateChanged(_)),
            "and it is named as an agent transition: {recorded:?}",
        );
        assert!(
            journal_events(&channels, "b", cursor_b).is_empty(),
            "the session whose answer did not move records nothing, exactly as it is not woken",
        );
    }

    /// The record lands at the revision the WAKE carries, not one behind it.
    ///
    /// A client parks at `R`, is answered `R'` by the bump, and asks for `(R, R']`. A record keyed
    /// `R` would be invisible to that read and would never be offered again — the client's cursor
    /// has already passed it. This is what `ChannelRegistry::announce` holds the journal lock for.
    #[test]
    fn the_record_is_readable_from_the_cursor_the_wake_answers() {
        let reg = registry_with(&[("a", claude_pane())]);
        let agents = clock();
        let channels = ChannelRegistry::default();
        let base = Instant::now();

        sweep_once(&reg, &agents, &channels, base, true);
        // What a parked client would be holding: the revision before the publishing pass.
        let parked_at = channels.revision("a").current();

        sweep_once(&reg, &agents, &channels, base + DEFAULT_SETTLE, true);

        // What the wake answers it with.
        let woken_at = channels.revision("a").current();
        assert!(woken_at > parked_at, "the settle advanced the scene");
        assert_eq!(
            journal_events(&channels, "a", parked_at).len(),
            1,
            "the record is inside the window the client asks for",
        );
        assert!(
            journal_events(&channels, "a", woken_at).is_empty(),
            "and is delivered once — a reader level with the wake has already accounted for it",
        );
    }

    /// Everything `session`'s journal has recorded above `cursor`.
    fn journal_events(channels: &ChannelRegistry, session: &str, cursor: u64) -> Vec<Event> {
        channels
            .journal(session)
            .lock()
            .expect("the journal")
            .since(cursor)
            .events
    }
}
