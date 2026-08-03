//! Which JOB owns each pane's terminal, remembered between sweeps so that a change in it can be an
//! EVENT rather than something every reader has to go and look for.
//!
//! # The fact, and why watching it is affordable
//!
//! A shell hands its terminal to the job the user runs and takes it back when that job ends. That
//! handover is the OS's own record of "the user started something" and "the thing the user started
//! is over" — and the daemon cannot see it happen, because from its side a user running `cargo
//! build` is bytes on a pty.
//!
//! R290 published WHAT is running
//! ([`PaneProcesses`](sprag_terminal::PaneProcesses), the `pane_processes` address) and measured
//! what that answer costs: **2751 us for a fresh read**, because `/proc` has no index by process
//! group and naming a job's members means a pass over every process on the box. Nothing can be
//! watched at that price.
//!
//! Its IDENTITY is a different object.
//! [`PanePty::foreground_pgid`](sprag_terminal::PanePty::foreground_pgid) is one
//! `/proc/<pid>/stat` read — one line, through the crate's single parser of it — and a change in
//! that number IS a job change. So the daemon watches the identity and lets the reader pay for the
//! description only when it wants one.
//!
//! **That is the whole reason this module exists rather than a `pane_processes` poll.** A watch
//! over the answer would be 2751 us every five seconds forever; a watch over its identity is a
//! `read(2)` per pane.
//!
//! # Why it is a TYPE and not a map inside the sweep
//!
//! The same reason [`AgentRegistry`](crate::AgentRegistry) is one. Two properties have to be
//! testable in isolation, and neither is reachable through a local in a loop:
//!
//! * **A first reading ESTABLISHES; it does not change.** A pane nobody has sampled goes from
//!   *unknown* to *some group*, and reporting that would announce a job change for every pane on
//!   the first sweep after boot and for every pane on the sweep after it is born. The rule has two
//!   neighbours already: [`Event::PaneSelected`](crate::Event::PaneSelected) is not emitted when a
//!   window gains its FIRST active pane, and
//!   [`Event::AgentStateChanged`](crate::Event::AgentStateChanged) is not emitted for a candidate.
//! * **A tracker must not outlive its pane.** The census is daemon-wide and the prune is
//!   [`retain_live`](JobWatch::retain_live), for the reason its counterpart states: a walk over one
//!   session would forget every other session's panes.

use std::collections::{HashMap, HashSet};
use std::sync::Mutex;

use sprag_terminal::PaneId;

use crate::external::lock;

/// The last reading of every pane's foreground job, and the rule for what counts as a change.
///
/// Shared by reference and internally locked, exactly as [`AgentClock`](crate::AgentClock) is: the
/// settle waker is the only writer today, and a reader that wants the daemon's own last answer
/// should not have to be handed a `&mut`.
///
/// # What a reading is
///
/// `Option<u32>` — a process group id, or `None` for a pane whose terminal nothing owns. `None` is
/// a real reading and not an absence: a pane whose child has exited answers it (`PanePty::pid`
/// stops answering once the exit is published, so `foreground_pgid` follows it down), and the move
/// INTO it is the first and only notice a reader gets that the pane's child is gone.
///
/// Never having sampled a pane is the absence of its key, which is a different thing, and keeping
/// the two distinguishable is what [`observe`](Self::observe) rests on.
#[derive(Debug, Default)]
pub struct JobWatch {
    seen: Mutex<HashMap<PaneId, Option<u32>>>,
}

impl JobWatch {
    /// An empty watch — every pane unknown, so every pane's next reading establishes it.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Record what a pane's foreground job is NOW, and answer whether that is a CHANGE.
    ///
    /// `false` for a pane this has never seen, which is [the establish rule](self) and the reason
    /// the previous reading is read out of the insert rather than looked up first: `HashMap::insert`
    /// answers `None` for a key that was absent and `Some(previous)` for one that was not, so
    /// "never sampled" and "sampled, and nothing owned the terminal" cannot be confused by any
    /// reading of this function — they are different arms.
    ///
    /// `true` only when a reading this watch already had has moved. A pane that keeps its job
    /// across a thousand sweeps is a thousand map writes and no events.
    pub fn observe(&self, pane: PaneId, reading: Option<u32>) -> bool {
        match lock(&self.seen).insert(pane, reading) {
            // Never sampled. This reading is the pane's first, so there is nothing for it to differ
            // from and nothing to announce.
            None => false,
            // Sampled before, so the comparison is meaningful — including `Some(None)`, a pane whose
            // terminal was already owned by nothing.
            Some(previous) => previous != reading,
        }
    }

    /// Forget every pane not in `live`.
    ///
    /// `live` must be the DAEMON-WIDE pane set. A census taken from one session would forget every
    /// other session's panes and re-establish them on the next sweep, which would then announce
    /// nothing — so the failure would be a silently missed event, not a visible one.
    pub fn retain_live(&self, live: &HashSet<PaneId>) {
        lock(&self.seen).retain(|pane, _| live.contains(pane));
    }

    /// How many panes this watch remembers — the observable the prune is asserted through.
    #[must_use]
    pub fn len(&self) -> usize {
        lock(&self.seen).len()
    }

    /// Whether it remembers nothing.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        lock(&self.seen).is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// **THE establish rule.** A pane's first reading is not a change, and the very next one is if
    /// it differs.
    ///
    /// The second half is the CONTROL: without it this passes on a watch that answers `false` to
    /// everything, which is the shape the bug would actually take.
    #[test]
    fn a_first_reading_establishes_and_the_next_one_can_change() {
        let watch = JobWatch::new();
        let pane = PaneId(0);

        assert!(
            !watch.observe(pane, Some(4242)),
            "nobody had asked about this pane, so there is nothing for the reading to differ from",
        );
        assert!(
            watch.observe(pane, Some(4343)),
            "CONTROL: a reading it already had, moved, IS the event",
        );
        assert!(
            !watch.observe(pane, Some(4343)),
            "and the same job across two sweeps is not — which is every quiet pane, every sweep",
        );
    }

    /// A pane whose terminal nothing owns is a READING, not an absence — so both edges into and out
    /// of it are changes.
    ///
    /// This is the transition a reader learns a pane's child died from: `PanePty::pid` stops
    /// answering once the exit is published, `foreground_pgid` follows it to `None`, and a dead pane
    /// keeps its place so `PaneClosed` never fires.
    #[test]
    fn losing_the_job_is_a_change_and_so_is_getting_one_back() {
        let watch = JobWatch::new();
        let pane = PaneId(0);

        assert!(!watch.observe(pane, Some(4242)));
        assert!(
            watch.observe(pane, None),
            "the job ended — the one notice a reader gets that this pane's child is gone",
        );
        assert!(
            !watch.observe(pane, None),
            "and it stays gone, which is not news twice",
        );
        assert!(
            watch.observe(pane, Some(5150)),
            "CONTROL: a pane can be restored to a job, and that is a change too",
        );
    }

    /// **The confusion the map's shape exists to prevent.** A pane whose FIRST reading is `None`
    /// must establish silently, exactly as one whose first reading is a real group does.
    ///
    /// A watch that stored a bare `Option<u32>` and read a missing key as `None` would answer
    /// `false` here and `true` on the line after it — reporting a job change on a pane that never
    /// had one, and staying silent when it got one.
    #[test]
    fn a_first_reading_of_no_job_establishes_too() {
        let watch = JobWatch::new();
        let pane = PaneId(0);

        assert!(
            !watch.observe(pane, None),
            "unknown and 'nothing owns it' are different facts, and this is the first",
        );
        assert!(
            watch.observe(pane, Some(4242)),
            "CONTROL: the pane then gets a job, and THAT is the change",
        );
    }

    /// Panes are tracked independently: one pane's change says nothing about another's.
    #[test]
    fn each_pane_is_watched_on_its_own() {
        let watch = JobWatch::new();

        assert!(!watch.observe(PaneId(0), Some(4242)));
        assert!(!watch.observe(PaneId(1), Some(4242)));
        assert!(watch.observe(PaneId(0), Some(9)));
        assert!(
            !watch.observe(PaneId(1), Some(4242)),
            "pane 1 never moved, and pane 0 sharing its group number is not pane 1's business",
        );
        assert_eq!(watch.len(), 2);
    }

    /// A tracker must not outlive its pane, and a pane that comes BACK under a recycled id must
    /// establish again rather than being compared against a dead pane's job.
    ///
    /// The second half is what makes the prune load-bearing rather than tidy: pane ids are not
    /// reused today, but a watch that kept the entry would compare a fresh pane's first reading
    /// against a stranger's, and announce or swallow an event on that basis.
    #[test]
    fn the_watch_forgets_a_pane_that_is_gone() {
        let watch = JobWatch::new();
        watch.observe(PaneId(0), Some(4242));
        watch.observe(PaneId(1), Some(4343));
        assert_eq!(watch.len(), 2);

        watch.retain_live(&HashSet::from([PaneId(0)]));
        assert_eq!(
            watch.len(),
            1,
            "pane 1 is gone, and so is what it was running"
        );

        assert!(
            !watch.observe(PaneId(1), Some(7777)),
            "so a pane arriving under that id establishes, rather than being told it changed",
        );
    }

    /// An empty watch remembers nothing — the `Default` the daemon and every test start from.
    #[test]
    fn a_new_watch_remembers_nothing() {
        let watch = JobWatch::new();
        assert!(watch.is_empty());
        assert_eq!(watch.len(), 0);
    }
}
