//! The daemon's agent-state memory: one [`Tracker`] per pane, and the manifest list they share.
//!
//! H3 slice 3. [`sprag_detect`] answers from one screen and one title, and [`Tracker`] adds what a
//! single frame cannot know (the settle window, the quiescence gate, the identity a modal covers).
//! This module is where those trackers LIVE, and the three reasons it is here rather than anywhere
//! more obvious are all facts about this daemon rather than preferences:
//!
//! * **Not on `sprag_terminal::Pane`.** The producer crate depends on the emulator and the PTY and
//!   nothing else, on purpose; `sprag-grid` — the pure-read crate H3's design cites as its exact
//!   precedent — is likewise consumed on this side of that line and never by the producer. A
//!   detector is a scene fact.
//! * **Not on [`WorkspaceExternal`](crate::WorkspaceExternal).** That external is rebuilt for every
//!   JSON-RPC request, so a map held as one of its fields would be born empty and dropped per poll.
//!   The hysteresis would be inert while every individual verdict still looked right, which is the
//!   worst available failure: silent, and invisible to any test that reads one verdict.
//! * **Keyed by [`PaneId`] alone**, which is sound because every window's `Workspace` in a daemon
//!   draws ids from ONE shared counter — so an id names a pane across the whole daemon, not within a
//!   session. That is what lets one registry serve every session and lets a pane keep its memory
//!   across a break/join.
//!
//! # Who drives it
//!
//! Two callers, one tracker, no second path to a verdict:
//!
//! * The **pane-list query** observes every pane it is about to describe, inside the workspace lock
//!   it already holds. That is what makes a verdict resting on present evidence — a dialog — reach a
//!   person on the output that painted it.
//! * The **settle waker** observes the panes whose [`Tracker::pending_deadline`] has passed. Without
//!   it, a verdict resting on an ABSENCE could never confirm itself: the pane list is served when a
//!   client asks, a client asks when the scene revision moves, and the last thing to move it was the
//!   output that STOPPED. See `sprag-term`'s waker for the whole argument.
//!
//! Both calls go through [`AgentRegistry::observe`], so there is one arbitration and one memory. The
//! second call in a pass is a no-op by construction — the quiescence gate skips it, and a pending
//! candidate's window is measured from when it was first seen rather than from the last look — and
//! `two_observations_of_one_unchanged_pane_publish_once` asserts that rather than trusting it.

use std::collections::{HashMap, HashSet};
use std::sync::{Condvar, Mutex, PoisonError};
use std::time::{Duration, Instant};

use sprag_detect::{Hysteresis, Ruleset, Tracker};
use sprag_terminal::PaneId;
use sprag_vt::Screen;

use crate::external::lock;

/// One pane's published agent state, in the shape the pane list puts on the wire.
///
/// Produced only for a pane with a KNOWN state: [`AgentRegistry::observe`] returns `None` where the
/// verdict is `Unknown`, so D8's additive rule — the key is absent entirely for a pane no manifest
/// claims — is carried by the type instead of by a caller remembering to check. A workspace with no
/// agents cannot accidentally grow a key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentFacts {
    /// `working` / `blocked` / `idle`, from [`sprag_detect::AgentState::wire_str`] — never a
    /// spelling invented here.
    pub state: &'static str,
    /// Which manifest claims the pane, `None` while a rule fired but no manifest is identified.
    pub agent: Option<String>,
    /// Which rule fired. D7: a gate that cannot say what it saw cannot be diagnosed, and this is
    /// what `explain` reads — it is the same value the detector produced, not a recomputation.
    pub rule: Option<String>,
    /// Increments on a published CHANGE, so a client tells "still blocked" from "blocked again"
    /// without diffing strings — `notification_seq`'s treatment.
    pub seq: u64,
}

/// Every pane's agent-state memory, plus the one ruleset they are all evaluated against.
#[derive(Debug, Default)]
pub struct AgentRegistry {
    /// Compiled ONCE per edit of the user's config, not per evaluation. A caller that rebuilt this
    /// on the hot path would recompile every pattern of every agent on a path served once per client
    /// wake, which is why [`sprag_detect::built_ins`] is a named list rather than a literal at each
    /// use and why [`crate::config::AgentManifests`] holds the file rather than reading it.
    rules: Ruleset,
    trackers: HashMap<PaneId, Tracker>,
}

impl AgentRegistry {
    /// A registry over `rules`, with no pane remembered yet.
    ///
    /// Takes the ruleset rather than calling [`sprag_detect::built_ins`] itself, which is what lets
    /// the user's `config.toml` manifests layer over the built-ins without this type learning about
    /// files: [`crate::config::AgentManifests`] does the reading, and hands the result in.
    #[must_use]
    pub fn new(rules: Ruleset) -> Self {
        Self {
            rules,
            trackers: HashMap::new(),
        }
    }

    /// Swap in a ruleset the user has just edited, KEEPING every pane's memory.
    ///
    /// The trackers survive on purpose, and the two halves of the memory survive for different
    /// reasons. `seq` must not restart, or every client would read a manifest edit as a state change
    /// on every pane at once. The remembered IDENTITY survives because R252 made it a manifest NAME
    /// rather than a position in the list, in as many words *because slice 4 reloads that list from a
    /// file and a name survives a reload* — so a `codex` pane whose modal is covering its fingerprint
    /// is still a `codex` pane after the edit.
    ///
    /// What does NOT survive is the quiescence skip: the new ruleset carries a new
    /// [`Ruleset::revision`](sprag_detect::Ruleset::revision), which is one of the key's terms, so
    /// every remembered pane is [`stale`](Self::stale) until it has been looked at again.
    pub fn reload(&mut self, rules: Ruleset) {
        self.rules = rules;
    }

    /// Whether this pane's published verdict was reached under a ruleset that has since been
    /// replaced.
    ///
    /// The waker's third reason to ask about a pane, beside due and unknown. A pane can be settled,
    /// known, and waiting on nothing, and still owe an evaluation — because the input that moved was
    /// not on its screen. A pane nobody has ever observed is NOT stale; it is unknown, which is a
    /// different question with a different answer.
    #[must_use]
    pub fn stale(&self, id: PaneId) -> bool {
        self.trackers
            .get(&id)
            .and_then(Tracker::evaluated_under)
            .is_some_and(|revision| revision != self.rules.revision())
    }

    /// Take a reading of one pane and return what is published for it, or `None` for a pane no
    /// manifest claims.
    ///
    /// # When `window` is called
    ///
    /// At most once, and only when this pane will actually consult the settle window — three cases,
    /// and the third was found by a test rather than reasoned out:
    ///
    /// 1. A pane never seen before, whose tracker has to be built with a window.
    /// 2. A pane with a candidate ALREADY waiting, so this look's publish-or-not decision is made
    ///    against the window the user currently has set.
    /// 3. A pane whose candidate this look has just CREATED. It has not consulted the window yet, and
    ///    it is about to hand the waker a deadline to sleep on; leaving that deadline derived from
    ///    whatever the policy happened to be would make a `set-option` take effect one wake late — and
    ///    for a pane going quiet, that wake is the only one coming.
    ///
    /// A pane that has published and is waiting on nothing does not call it at all, which is the
    /// steady state of every settled pane in the workspace.
    ///
    /// That matters because the window is a user OPTION and this project's options are read from the
    /// file on every call — the daemon is a reader of the user's config, not a holder of it, so
    /// `set-option` needs nothing restarted. `config::window_size` prices that honestly as one file
    /// read per WINDOW CHANGE, which is a rare-event justification; this path is served on every
    /// client wake, so the same read placed unconditionally would be a file read per output batch per
    /// session. Tying it to "somebody is actually waiting on the window" keeps the quiet workspace at
    /// zero reads without caching a control the user expects to be live.
    pub fn observe(
        &mut self,
        id: PaneId,
        screen: &Screen,
        title: Option<&str>,
        now: Instant,
        window: impl FnOnce() -> Hysteresis,
    ) -> Option<AgentFacts> {
        // `window` is `FnOnce`, so the option is read at most once however many of the three cases
        // apply to this call.
        let mut source = Some(window);
        let tracker = match self.trackers.entry(id) {
            std::collections::hash_map::Entry::Occupied(entry) => {
                let tracker = entry.into_mut();
                if tracker.pending_deadline().is_some()
                    && let Some(read) = source.take()
                {
                    tracker.set_policy(read());
                }
                tracker
            }
            std::collections::hash_map::Entry::Vacant(entry) => {
                let read = source.take().expect("the source is untouched on this arm");
                entry.insert(Tracker::new(read()))
            }
        };
        let verdict = tracker.observe(screen, title, &self.rules, now);
        let state = verdict.state.wire_str();
        let agent = verdict.agent.clone();
        let rule = verdict.rule.clone();
        // Case 3: this look created the candidate, so the deadline it now implies has never been
        // measured against the user's window. Correct it before anyone sleeps on it.
        if tracker.pending_deadline().is_some()
            && let Some(read) = source.take()
        {
            tracker.set_policy(read());
        }
        Some(AgentFacts {
            state: state?,
            agent,
            rule,
            seq: tracker.seq(),
        })
    }

    /// When the earliest waiting candidate would publish, or `None` when nothing is waiting.
    ///
    /// This is the whole of what the settle waker needs to know to sleep: with nothing pending there
    /// is no clock to serve, which is what keeps M3's "a quiet workspace costs nothing" true of the
    /// confirmation and not only of the evaluation.
    #[must_use]
    pub fn next_deadline(&self) -> Option<Instant> {
        self.trackers
            .values()
            .filter_map(Tracker::pending_deadline)
            .min()
    }

    /// Whether this pane has a candidate whose window has closed by `now` — the waker's test for
    /// "this one needs asking, the rest do not".
    #[must_use]
    pub fn is_due(&self, id: PaneId, now: Instant) -> bool {
        self.trackers
            .get(&id)
            .and_then(Tracker::pending_deadline)
            .is_some_and(|deadline| deadline <= now)
    }

    /// Whether ANY pane's window has closed by `now`.
    ///
    /// The waker's guard against a pointless walk: it is woken the moment a candidate APPEARS (so it
    /// can re-plan its sleep around a nearer deadline), and that wake is not a reason to take every
    /// workspace lock — the deadline is still ahead. Cheap, because it reads only what it already
    /// holds.
    #[must_use]
    pub fn any_due(&self, now: Instant) -> bool {
        self.next_deadline().is_some_and(|deadline| deadline <= now)
    }

    /// Whether this pane has ever been observed.
    ///
    /// The sweep's test for "this one has no tracker yet, so nothing about it can be waiting". A
    /// candidate is only ever created by an observation, so a pane nobody has looked at has no state
    /// and no deadline — it is invisible to every other method here, which is why discovering it needs
    /// a question of its own.
    #[must_use]
    pub fn knows(&self, id: PaneId) -> bool {
        self.trackers.contains_key(&id)
    }

    /// The published `seq` for a pane, or 0 for one never observed.
    ///
    /// The waker compares this across an [`observe`](Self::observe) to learn whether the verdict
    /// MOVED, and so whether the session's clients have anything to be woken for. Reading the seq
    /// rather than re-deriving "did it change" keeps one definition of a published change.
    #[must_use]
    pub fn seq(&self, id: PaneId) -> u64 {
        self.trackers.get(&id).map_or(0, Tracker::seq)
    }

    /// Forget every pane not in `live`.
    ///
    /// **`live` must be a DAEMON-WIDE census.** The pane-list query walks one session's panes, and
    /// pruning against that would forget every other session's — so the hot path never calls this.
    /// The waker does, because it already walks every session to find due panes, so the census is a
    /// by-product of work it is doing anyway.
    ///
    /// Without this a tracker outlives its pane and the map grows for the life of the daemon, one
    /// entry per pane ever opened. Bounded memory is the whole of the requirement; the latency of
    /// forgetting is irrelevant, which is why it rides the waker's cadence rather than the query's.
    pub fn retain_live(&mut self, live: &HashSet<PaneId>) {
        self.trackers.retain(|id, _| live.contains(id));
    }

    /// How many panes are remembered — for the test that pruning happens at all.
    #[must_use]
    pub fn len(&self) -> usize {
        self.trackers.len()
    }

    /// Whether nothing is remembered.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.trackers.is_empty()
    }
}

/// The [`AgentRegistry`] plus the signal that says a deadline now exists — the shared handle every
/// caller holds.
///
/// # Why the signal cannot live apart from the memory
///
/// This type exists because the first version of the waker had D9's own bug one level up, and it took
/// a live drive against the daemon to see it. The waker computed its sleep from whatever was pending
/// when it last LOOKED. At daemon start nothing is pending, so it slept for the prune interval — and
/// then a pane-list query created a candidate with a two-second deadline that the sleeping thread had
/// no way to hear about. The mechanism built to confirm verdicts nothing else would confirm was itself
/// waiting for an event nothing produced.
///
/// The fix is not a shorter sleep. A waker that woke every few hundred milliseconds to ask "is
/// anything pending yet" would take every workspace lock several times a second forever on an idle
/// daemon, which is exactly the cost M3 measured away and R218–R220 built the projection and the fetch
/// to avoid. So the appearance of a candidate is an EVENT, and the thread waits on it: with nothing
/// pending the waker is blocked and costs nothing at all, and it learns about a new deadline at the
/// instant the deadline is created.
///
/// Pairing the [`Condvar`] with the [`Mutex`] in one type is what makes that unforgettable. A caller
/// holding the two separately can lock, create a candidate, and neglect to signal — and the failure
/// is silent, because every individual verdict still looks right. Here [`observe`](Self::observe) is
/// the only way to create a candidate and it signals for you.
///
/// # Why a RELOAD does not signal, when a candidate does
///
/// A reload gives every remembered pane an evaluation to owe, so the symmetrical shape would be a
/// `reload` here that takes the lock and notifies. It would be signalling NOBODY. The only reloader
/// is the waker itself, re-reading the user's file in its own sweep — and a thread cannot notify
/// itself: it is awake, its `sweep` is true, and the walk that follows serves the stale panes in the
/// same pass. A signal there would be a mechanism with no event, which is the defect this type
/// exists to prevent rather than a second instance of it to ship.
///
/// So the reload goes through [`with`](Self::with) like the waker's other work. If one ever arrives
/// from ANOTHER thread, a wake is not enough on its own: the loop also needs a reason to do work when
/// nothing is due and no sweep is owed. That reason lives in `sprag-term`'s `ask` predicate, which is
/// where "this pane owes an evaluation" is decided, and [`AgentRegistry::stale`] is the question it
/// asks.
#[derive(Debug, Default)]
pub struct AgentClock {
    state: Mutex<AgentRegistry>,
    /// Signalled when a pane gains a pending candidate. The waker's `wait_timeout` parks on this, so
    /// "there is now a deadline" reaches it without a poll.
    appeared: Condvar,
}

impl AgentClock {
    /// A clock over `rules`, with no pane remembered yet.
    #[must_use]
    pub fn new(rules: Ruleset) -> Self {
        Self {
            state: Mutex::new(AgentRegistry::new(rules)),
            appeared: Condvar::new(),
        }
    }

    /// [`AgentRegistry::observe`], signalling the waker if this look CREATED a deadline.
    ///
    /// The signal is on the edge — not pending before, pending after — rather than on every look at a
    /// pending pane. A repainting pane would otherwise notify on every client wake, and each of those
    /// notifications costs the waker a trip round its loop to conclude that the deadline it already
    /// knew about has not moved.
    pub fn observe(
        &self,
        id: PaneId,
        screen: &Screen,
        title: Option<&str>,
        now: Instant,
        window: impl FnOnce() -> Hysteresis,
    ) -> Option<AgentFacts> {
        let mut state = lock(&self.state);
        let before = state.next_deadline();
        let facts = state.observe(id, screen, title, now, window);
        let after = state.next_deadline();
        drop(state);
        if after != before && after.is_some() {
            self.appeared.notify_all();
        }
        facts
    }

    /// Read the memory under its lock — the waker's walk, and nothing else.
    pub fn with<R>(&self, f: impl FnOnce(&mut AgentRegistry) -> R) -> R {
        f(&mut lock(&self.state))
    }

    /// Park until there is a deadline to serve, or until `cap` elapses, whichever comes first —
    /// returning EARLY if a candidate appears in the meantime.
    ///
    /// `cap` bounds the wait so the caller still gets a turn for work that is not deadline-driven
    /// (forgetting the panes that are gone). With a deadline already past this returns at once, so a
    /// caller that has fallen behind does not add a wait to the lateness.
    pub fn park_until_due(&self, cap: Duration) {
        let state = lock(&self.state);
        let wait = state
            .next_deadline()
            .map(|deadline| deadline.saturating_duration_since(Instant::now()))
            .filter(|remaining| *remaining < cap)
            .unwrap_or(cap);
        if wait.is_zero() {
            return;
        }
        // A spurious wake is harmless: the caller re-reads `any_due` and parks again.
        let _unused = self
            .appeared
            .wait_timeout(state, wait)
            .unwrap_or_else(PoisonError::into_inner);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sprag_detect::DEFAULT_SETTLE;
    use sprag_vt::{Emulator, VtPort};

    /// A `claude` pane the footer fingerprint claims, with no title at all — the smallest screen
    /// these tests need, since the RULES' fidelity to a real agent is slice 1's business.
    const CLAUDE_FOOTER: &[&str] = &["❯", "  ⏸ manual mode on · ? for shortcuts"];

    /// The smallest screen the dialog rule fires on.
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

    /// The window is read for a pane never seen and for one with a candidate waiting, and NOT for a
    /// pane that has published and is waiting on nothing — the property that keeps a file read off the
    /// hot path. COUNTED, because "we only read it when needed" is exactly the kind of claim that
    /// quietly stops being true.
    ///
    /// Note what the sequence below had to be corrected to say: a FIRST sighting of a resting pane
    /// leaves a candidate pending, because the settle window applies to a first publication too. A
    /// pane is not settled because it has been looked at once.
    #[test]
    fn the_settle_window_is_read_only_when_a_pane_will_consult_it() {
        let mut reg = AgentRegistry::default();
        let mut em = painted(CLAUDE_FOOTER);
        let base = Instant::now();
        let mut reads = 0_u32;

        let observe = |reg: &mut AgentRegistry, em: &Emulator, now, reads: &mut u32| {
            reg.observe(PaneId(1), em.screen(), Some("✳ Claude Code"), now, || {
                *reads += 1;
                Hysteresis::default()
            })
        };

        // First sighting: the tracker has to be built, so the window is read.
        observe(&mut reg, &em, base, &mut reads);
        assert_eq!(reads, 1, "a pane never seen needs a window to build with");

        // That first look left `Idle` waiting, so the window is live and read again — and this look
        // publishes it.
        let settled = base + DEFAULT_SETTLE;
        observe(&mut reg, &em, settled, &mut reads);
        assert_eq!(reads, 2, "a waiting candidate consults the window");
        assert_eq!(reg.next_deadline(), None, "and this look published it");

        // NOW the pane is settled: published, nothing pending, nothing moved. No read.
        observe(&mut reg, &em, settled, &mut reads);
        observe(&mut reg, &em, settled, &mut reads);
        assert_eq!(reads, 2, "a settled, known pane does not touch the file");

        // A dialog publishes on sight, so it leaves nothing waiting and needs no window either.
        repaint(&mut em, DIALOG);
        observe(&mut reg, &em, settled, &mut reads);
        assert_eq!(reads, 2, "an active verdict is published without a window");

        // The dialog goes: a return to rest is an absence, so a candidate waits and the window is
        // live again.
        repaint(&mut em, CLAUDE_FOOTER);
        observe(&mut reg, &em, settled, &mut reads);
        assert_eq!(reads, 3, "a fresh candidate re-reads the user's window");
    }

    /// D8's additive rule, carried by the return type: a pane no manifest claims produces nothing at
    /// all, so the wire shape for a workspace without agents cannot drift.
    #[test]
    fn a_pane_no_manifest_claims_produces_no_facts() {
        let mut reg = AgentRegistry::default();
        let em = painted(&["$ ls", "file.txt"]);
        let facts = reg.observe(
            PaneId(7),
            em.screen(),
            None,
            Instant::now(),
            Hysteresis::default,
        );
        assert_eq!(facts, None);
    }

    /// Both callers — the query and the waker — go through one tracker, so a second look at an
    /// unchanged pane must change nothing. Two clients polling one wake is exactly this.
    #[test]
    fn two_observations_of_one_unchanged_pane_publish_once() {
        let mut reg = AgentRegistry::default();
        let mut em = painted(CLAUDE_FOOTER);
        let base = Instant::now();
        // The footer has to be SEEN before the dialog covers it, or the pane is claimed by nothing
        // and the dialog is a choice list in an anonymous pane — R251's finding, and the reason the
        // identity memory exists.
        reg.observe(PaneId(1), em.screen(), None, base, Hysteresis::default);
        repaint(&mut em, DIALOG);

        let first = reg
            .observe(PaneId(1), em.screen(), None, base, Hysteresis::default)
            .expect("the dialog rule fires");
        let second = reg
            .observe(PaneId(1), em.screen(), None, base, Hysteresis::default)
            .expect("and again");

        assert_eq!(first.state, "blocked");
        assert_eq!(first, second, "the second look is not a second publication");
        assert_eq!(reg.seq(PaneId(1)), 1);
    }

    /// The waker's two questions, against the same pane: when to come back, and who is due now.
    #[test]
    fn a_waiting_candidate_is_due_at_its_deadline_and_not_before() {
        let mut reg = AgentRegistry::default();
        let mut em = painted(CLAUDE_FOOTER);
        let base = Instant::now();

        repaint(&mut em, DIALOG);
        reg.observe(PaneId(1), em.screen(), None, base, Hysteresis::default);
        assert_eq!(
            reg.next_deadline(),
            None,
            "a verdict published on sight leaves the waker nothing to do",
        );

        repaint(&mut em, CLAUDE_FOOTER);
        reg.observe(PaneId(1), em.screen(), None, base, Hysteresis::default);
        assert_eq!(reg.next_deadline(), Some(base + DEFAULT_SETTLE));
        assert!(!reg.is_due(PaneId(1), base + DEFAULT_SETTLE / 2));
        assert!(reg.is_due(PaneId(1), base + DEFAULT_SETTLE));

        // And the seq moves only when the waker's own observation publishes.
        let before = reg.seq(PaneId(1));
        reg.observe(
            PaneId(1),
            em.screen(),
            None,
            base + DEFAULT_SETTLE,
            Hysteresis::default,
        );
        assert_eq!(
            reg.seq(PaneId(1)),
            before + 1,
            "the waker published the rest"
        );
        assert_eq!(reg.next_deadline(), None);
    }

    /// A tracker must not outlive its pane, or the map grows for the life of the daemon.
    #[test]
    fn a_pane_absent_from_the_census_is_forgotten() {
        let mut reg = AgentRegistry::default();
        let em = painted(CLAUDE_FOOTER);
        let now = Instant::now();
        for id in [1, 2, 3] {
            reg.observe(PaneId(id), em.screen(), None, now, Hysteresis::default);
        }
        assert_eq!(reg.len(), 3);

        reg.retain_live(&HashSet::from([PaneId(1), PaneId(3)]));
        assert_eq!(reg.len(), 2, "the pane the census did not mention is gone");
        assert_eq!(
            reg.seq(PaneId(2)),
            0,
            "and it is forgotten, not merely hidden"
        );
    }
}

#[cfg(test)]
mod clock_tests {
    use super::*;
    use std::sync::Arc;

    use sprag_detect::DEFAULT_SETTLE;
    use sprag_vt::{Emulator, VtPort};

    const CLAUDE_FOOTER: &[&str] = &["❯", "  ⏸ manual mode on · ? for shortcuts"];

    fn painted(lines: &[&str]) -> Emulator {
        let mut em = Emulator::new(80, 24);
        em.advance(lines.join("\r\n").as_bytes());
        em
    }

    /// The defect this type exists for, as a test: a parked waker must learn about a candidate that
    /// appears AFTER it parked.
    ///
    /// The first waker computed its sleep once and slept for the prune interval, because at daemon
    /// start nothing is pending — and the query that created a two-second candidate had no way to
    /// reach it. That shipped-looking, fully-reviewed thread published nothing at all, and only a live
    /// drive against the binary showed it.
    ///
    /// So the assertion is about TIME: park with a cap far longer than the test's patience, create a
    /// candidate from another thread, and require the park to return anyway. Remove the `notify_all`
    /// in `observe` and this blocks for the whole cap.
    #[test]
    fn a_parked_waker_returns_when_a_candidate_appears() {
        let clock = Arc::new(AgentClock::default());
        // Far longer than any deadline in this test, so a park that returns can only have been woken.
        let cap = Duration::from_secs(60);

        let parked = Arc::clone(&clock);
        let (tx, rx) = std::sync::mpsc::channel();
        let waker = std::thread::spawn(move || {
            parked.park_until_due(cap);
            let _ = tx.send(());
        });

        // Give the thread time to actually be inside the wait before the candidate appears. If it has
        // not parked yet the test still passes for the right reason — the park would compute the
        // deadline it can now see — so this sleep cannot make a broken implementation look correct.
        std::thread::sleep(Duration::from_millis(50));

        let em = painted(CLAUDE_FOOTER);
        let facts = clock.observe(
            PaneId(1),
            em.screen(),
            Some("✳ Claude Code"),
            Instant::now(),
            Hysteresis::default,
        );
        assert_eq!(facts, None, "a resting verdict does not publish on sight");
        assert!(
            clock.with(|state| state.next_deadline()).is_some(),
            "and it leaves a deadline for the waker to serve",
        );

        rx.recv_timeout(Duration::from_secs(5))
            .expect("the parked waker was never told a candidate appeared");
        waker.join().expect("the waker thread");
    }

    /// A park with a deadline already past must not add a wait to the lateness.
    #[test]
    fn a_park_returns_at_once_when_a_deadline_has_already_passed() {
        let clock = AgentClock::default();
        let em = painted(CLAUDE_FOOTER);
        // Dated in the PAST, so the candidate's window has already closed by the time anyone parks.
        let long_ago = Instant::now() - DEFAULT_SETTLE * 2;
        clock.observe(
            PaneId(1),
            em.screen(),
            Some("✳ Claude Code"),
            long_ago,
            Hysteresis::default,
        );
        assert!(clock.with(|state| state.any_due(Instant::now())));

        let start = Instant::now();
        clock.park_until_due(Duration::from_secs(60));
        assert!(
            start.elapsed() < Duration::from_millis(500),
            "a due deadline is served now, not after the cap: waited {:?}",
            start.elapsed(),
        );
    }

    /// The wake is on the EDGE. A pane already pending, looked at again, must not notify — otherwise a
    /// repainting pane sends the waker round its loop once per client wake for a deadline it already
    /// knows about.
    #[test]
    fn re_observing_an_already_pending_pane_does_not_wake_the_waker() {
        let clock = AgentClock::default();
        let em = painted(CLAUDE_FOOTER);
        let now = Instant::now();
        clock.observe(
            PaneId(1),
            em.screen(),
            Some("✳ Claude Code"),
            now,
            Hysteresis::default,
        );
        let deadline = clock.with(|state| state.next_deadline());
        assert!(
            deadline.is_some(),
            "the first look leaves a candidate waiting"
        );

        // Nothing has moved, so this look reaches the same candidate with the same `since`. A park
        // must therefore still be waiting on the same instant rather than having been woken.
        clock.observe(
            PaneId(1),
            em.screen(),
            Some("✳ Claude Code"),
            now,
            Hysteresis::default,
        );
        assert_eq!(
            clock.with(|state| state.next_deadline()),
            deadline,
            "the deadline did not move, so there was nothing to announce",
        );
    }

    /// `claude`, with its idle rule rewritten to conclude something else — a user's correction, in
    /// the smallest form that changes an answer.
    fn rewritten_claude() -> Ruleset {
        let mut claude = sprag_detect::claude();
        claude
            .rules
            .iter_mut()
            .find(|rule| rule.id == "idle-glyph")
            .expect("the built-in has an idle rule")
            .state = sprag_detect::AgentState::Working;
        Ruleset::new(vec![claude])
    }

    /// THE RELOAD CASE, and the one the waker's third `ask` reason exists for.
    ///
    /// A settled pane is not due and is not unknown, so under slice 3's two reasons nobody would
    /// ever look at it again. The input that moved is not on its screen — which is precisely the
    /// pane a user edits a manifest to fix, because a quiet pane is where a wrong verdict is visible
    /// and stuck.
    #[test]
    fn a_reload_makes_a_settled_pane_owe_an_evaluation() {
        let mut reg = AgentRegistry::default();
        let em = painted(CLAUDE_FOOTER);
        let id = PaneId(1);
        let title = Some("✳ Claude Code");
        let base = Instant::now();

        reg.observe(id, em.screen(), title, base, Hysteresis::default);
        let settled = reg
            .observe(
                id,
                em.screen(),
                title,
                base + DEFAULT_SETTLE,
                Hysteresis::default,
            )
            .expect("a claude pane publishes");
        assert_eq!(settled.state, "idle");
        assert!(
            !reg.stale(id),
            "this pane was evaluated under the rules that are in force",
        );

        reg.reload(rewritten_claude());
        assert!(
            reg.stale(id),
            "a replaced ruleset is an input the pane's own screen cannot report",
        );

        let after = reg
            .observe(
                id,
                em.screen(),
                title,
                base + DEFAULT_SETTLE * 2,
                Hysteresis::default,
            )
            .expect("still a claude pane");
        assert_eq!(
            after.state, "working",
            "the correction reaches a pane that has not painted a single row since",
        );
        assert!(!reg.stale(id), "and the debt is settled by looking");
    }

    /// The memory SURVIVES a reload, and the two halves survive for different reasons.
    ///
    /// `seq` must not restart, or a manifest edit would read on the wire as a state change on every
    /// pane at once. The identity must not be dropped, because R252 made it a manifest NAME rather
    /// than a position in the list *because slice 4 reloads that list from a file* — this is the
    /// round that gets to assert the reason was real.
    #[test]
    fn a_reload_keeps_the_seq_a_client_diffs_and_the_identity_a_modal_covers() {
        let mut reg = AgentRegistry::default();
        let em = painted(CLAUDE_FOOTER);
        let id = PaneId(1);
        let title = Some("✳ Claude Code");
        let base = Instant::now();

        reg.observe(id, em.screen(), title, base, Hysteresis::default);
        let settled = reg
            .observe(
                id,
                em.screen(),
                title,
                base + DEFAULT_SETTLE,
                Hysteresis::default,
            )
            .expect("published");

        reg.reload(rewritten_claude());
        let after = reg
            .observe(
                id,
                em.screen(),
                title,
                base + DEFAULT_SETTLE * 2,
                Hysteresis::default,
            )
            .expect("published");

        assert_eq!(
            after.seq,
            settled.seq + 1,
            "the seq counts on from what the client last read, rather than starting over",
        );
        assert_eq!(after.agent.as_deref(), Some("claude"));
        assert_eq!(
            reg.len(),
            1,
            "one pane, one tracker — the reload built no new map"
        );
    }

    /// A pane nobody has ever observed is UNKNOWN, not stale, and the waker asks about the two for
    /// different reasons. Collapsing them would make `stale` answer for panes it knows nothing
    /// about, which is the sweep's other job wearing this one's name.
    #[test]
    fn a_pane_nobody_has_observed_is_unknown_rather_than_stale() {
        let mut reg = AgentRegistry::default();
        let never = PaneId(7);
        assert!(!reg.knows(never));
        assert!(!reg.stale(never), "there is no earlier answer to be stale");

        reg.reload(rewritten_claude());
        assert!(
            !reg.stale(never),
            "and a reload does not invent one for a pane that was never looked at",
        );
    }
}
