//! A session's ACTIVITY — where it is working (`cwd`), on what (`branch`), and what it is serving
//! (`ports`) — sampled from the operating system, held with the time it was taken, and shared by
//! every reader that asks for it.
//!
//! # Why this is not part of the session list (R282)
//!
//! These three facts and the ones on [`SessionInfo`](crate::SessionInfo) are different KINDS of
//! fact, and until R282 they were served as one:
//!
//! | | the registry's structure | this |
//! |---|---|---|
//! | what | name, windows, panes, default | cwd, branch, ports |
//! | changes on | an event this daemon performs | nothing this daemon can see |
//! | so it is | published, and the scene revision carries it | SAMPLED, at some time, with some age |
//! | costs | a registry lock and a pool lock | the filesystem, and `/proc` for every process |
//!
//! Serving them together forced the sampled half to be re-taken whenever the published half might
//! have moved. A display client re-reads the session list on every poll wake, and a wake is a batch
//! of PTY output — so a character arriving in any pane cost a `/proc` walk of the whole box, per
//! attached client, for three facts that a printed character tells you nothing about. Measured on
//! the `sprag-latency` battery before the split: **1.257 us** for the list with no live pane against
//! **3478.178 us** with one, a difference of 20.9% of a 60 Hz frame, against 9.5 us for the pane
//! list a client actually draws from.
//!
//! # The shape that fixes it
//!
//! A sample is [ASKED FOR](ActivitySampler::read) with the staleness the caller will accept, and
//! answered with the age it actually has. That machinery — hold, coalesce, report the age — is
//! [`Sampled`], which this module is the first user of; what stays HERE is the part that is
//! about sessions, which is this module's own `sample` and its locking order.
//!
//! The cadence therefore lives in exactly one place — the tolerance a caller declares — instead of
//! being split between a client-side timer and a daemon-side cache that would have to agree.

use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use crate::registry::SessionRegistry;
use crate::sampled::Sampled;

/// One session's live ACTIVITY: where it is working, on what, and what it is serving.
///
/// Keyed by session [`name`](Self::name) rather than positioned against a session list, because the
/// two answers are read over separate requests and a list index would silently pair one read's rows
/// with another's. A name is a session's ADDRESS ([`SessionRegistry::new_session`] refuses a
/// duplicate for exactly that reason), unique by construction, so the join cannot go wrong.
///
/// Every field is derived HOST-side, so a display client carries the resulting strings and numbers
/// and never the `/proc` logic that produced them. A session whose current window holds no pane, or
/// a platform with no `/proc`, carries the honest empty rather than a guess.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SessionActivity {
    /// The session this row describes — its name, which is its address.
    pub name: String,
    /// The session's current window's FIRST pane's live working directory, in display form (lossy),
    /// or `None` when that pane is gone or the platform exposes no `/proc`.
    ///
    /// The first pane DELIBERATELY, now that a window also holds an active pane this could have
    /// used: a listing describes sessions the reader is mostly NOT in, and the oldest pane of the
    /// window they would see on attach is a stable representative — it does not move as somebody
    /// walks around inside that session, so `sprag ls` does not flicker between directories while a
    /// user navigates.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    /// The git branch checked out at [`cwd`](Self::cwd) (or a short `(sha)` for a detached HEAD),
    /// `None` outside a work tree.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    /// The distinct TCP ports any process in this session is LISTENING on, ascending — the cmux
    /// "what is this workspace serving" fact (a dev server on `:3000`).
    ///
    /// Aggregated over EVERY pane of ALL the session's windows, because a listening server usually
    /// runs in a different pane than the one whose [`cwd`](Self::cwd) is shown. Empty when the
    /// session serves nothing or the platform exposes no `/proc`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub ports: Vec<u16>,
}

/// A whole [`ActivitySampler`] reading: every session's activity, and how old the reading is.
///
/// One age for the whole reading, not one per row, because one pass produces them all: the `/proc`
/// walk that attributes listening sockets is shared across every session, so no row is fresher than
/// another. Why an age travels at all is [`Reading`](crate::Reading)'s own doc.
pub type ActivityReading = crate::Reading<Vec<SessionActivity>>;

/// The one place a session's [activity](SessionActivity) is sampled, and the one place a sample is
/// held between reads.
///
/// Shared (`Arc`) by every arm that serves the question — the wire slot and the in-process host —
/// so two readers can neither disagree about what a field means nor pay twice for the same walk.
///
/// A thin named wrapper over [`Sampled`], and the two halves are deliberately split there: the
/// generic holds the CACHE and the coalescing, this owns the QUESTION. Binding the sampling
/// function to the type is the point — a bare `Sampled<Vec<SessionActivity>>` would let two call
/// sites hand it two different sampling functions and cache two different facts in one slot.
///
/// # Locking
///
/// The sampler's own lock is taken FIRST and held across the sample, which acquires the registry
/// lock and then each pool lock in turn (never nested — [`SessionRegistry::session_infos_live`]'s
/// discipline, and this follows it for the same reason). So the order is sampler → registry → pool,
/// and nothing anywhere takes them in the other direction.
#[derive(Default)]
pub struct ActivitySampler {
    held: Sampled<Vec<SessionActivity>>,
}

impl ActivitySampler {
    /// A sampler holding nothing yet.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Every session's activity, no older than `max_age`, with the age it actually has.
    ///
    /// `max_age` is the caller's STALENESS TOLERANCE, and it is the only cadence control in the
    /// design: a one-shot human command (`sprag ls`) passes [`Duration::ZERO`] and waits for its own
    /// fresh walk, while a sidebar poll passes a window it can live with and is answered from the
    /// held sample for free. See [`Sampled::read`] for what that tolerance does and does not do.
    pub fn read(
        &self,
        registry: &Arc<Mutex<SessionRegistry>>,
        max_age: Duration,
    ) -> ActivityReading {
        self.held.read(max_age, || sample(registry))
    }
}

/// Take one reading of every session's activity — the expensive half, off every lock this crate
/// holds for longer than a moment.
///
/// TWO-PHASE, exactly like [`SessionRegistry::session_infos_live`] and for the same reason (the
/// module's registry-then-workspace, never-nested discipline):
///  1. under the registry lock: each session's name, its current window's pool (for the cwd) and all
///     its windows' pools (for the ports), in ONE pass so entry `i` of every `Vec` names the same
///     session;
///  2. lock RELEASED — the current pool locked on its own for its first pane's live cwd, and every
///     window pool locked on its own for its panes' child pids;
///  3. no lock — the git branch derived from the cwd, and the listening ports from the pids via ONE
///     shared `/proc` scan, so the walk is a single pass for the whole reading rather than one per
///     session.
fn sample(registry: &Arc<Mutex<SessionRegistry>>) -> Vec<SessionActivity> {
    let (names, current_pools, window_pools) = {
        let reg = registry.lock().unwrap_or_else(PoisonError::into_inner);
        let (names, current) = reg.current_pools();
        (names, current, reg.window_pools())
    };
    let cwds: Vec<_> = current_pools
        .iter()
        .map(|pool| {
            let pool = pool.lock().unwrap_or_else(PoisonError::into_inner);
            pool.panes().first().and_then(|pane| pane.pty().cwd())
        })
        .collect();
    let pids: Vec<Vec<u32>> = window_pools
        .iter()
        .map(|pools| SessionRegistry::window_pids(pools))
        .collect();
    // ONLY when some session actually holds a live pane. An idle daemon then pays no `/proc` walk
    // even for a caller that demanded a fresh sample; an empty scan reports no ports anyway, so the
    // skip changes the cost and not the answer.
    let scan = if pids.iter().any(|session| !session.is_empty()) {
        crate::ports::ProcScan::scan()
    } else {
        crate::ports::ProcScan::default()
    };
    names
        .into_iter()
        .zip(cwds)
        .zip(pids)
        .map(|((name, cwd), pids)| SessionActivity {
            branch: cwd.as_deref().and_then(crate::git::branch),
            cwd: cwd.map(|cwd| cwd.to_string_lossy().into_owned()),
            ports: scan.listening_ports(&pids),
            name,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::registry::SessionRegistry;

    /// A registry with one session and no pane — enough to exercise the sampler's own logic without
    /// a PTY, which is what these tests are about (the derivation itself is `ports`' and `git`'s).
    fn registry() -> Arc<Mutex<SessionRegistry>> {
        Arc::new(Mutex::new(SessionRegistry::new((80, 24))))
    }

    #[test]
    fn a_first_read_is_freshly_sampled_and_says_so() {
        let reading = ActivitySampler::new().read(&registry(), Duration::from_secs(3600));
        assert_eq!(
            reading.age,
            Duration::ZERO,
            "nothing was held, so the tolerance cannot be met by anything but a fresh sample",
        );
        assert_eq!(
            reading.value.len(),
            1,
            "one row per session in the registry"
        );
    }

    #[test]
    fn a_second_read_within_tolerance_is_answered_from_the_held_sample() {
        let sampler = ActivitySampler::new();
        let registry = registry();
        let first = sampler.read(&registry, Duration::from_secs(3600));
        assert_eq!(first.age, Duration::ZERO);
        // A second session, created AFTER the sample: a reading that shows it came from a fresh
        // walk, and one that does not came from the held sample. The control is the registry's own
        // content, not a clock nobody can pin.
        registry
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .new_session(Some("late"))
            .expect("a free name");
        let second = sampler.read(&registry, Duration::from_secs(3600));
        assert_eq!(
            second.value.len(),
            1,
            "the held sample predates the second session, so it cannot mention it",
        );
    }

    #[test]
    fn zero_tolerance_always_takes_a_fresh_sample() {
        let sampler = ActivitySampler::new();
        let registry = registry();
        sampler.read(&registry, Duration::ZERO);
        registry
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .new_session(Some("late"))
            .expect("a free name");
        let second = sampler.read(&registry, Duration::ZERO);
        assert_eq!(
            second.age,
            Duration::ZERO,
            "a caller admitting no staleness is answered by a sample taken for it",
        );
        assert_eq!(
            second.value.len(),
            2,
            "and that fresh sample sees the session the held one could not",
        );
    }

    #[test]
    fn every_session_gets_a_row_named_by_its_own_name() {
        let registry = registry();
        registry
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .new_session(Some("work"))
            .expect("a free name");
        let reading = ActivitySampler::new().read(&registry, Duration::ZERO);
        let names: Vec<&str> = reading.value.iter().map(|row| row.name.as_str()).collect();
        assert_eq!(names, ["0", "work"], "registry order, addressed by name");
    }
}
