//! **WHEN A PANE MOVED**, as a number a waiter can park on.
//!
//! # ⚠⚠⚠⚠ Why this exists: waiting was implemented as asking, a hundred times a second
//!
//! Everything in this workspace that waits for a pane to do something used to wait by ASKING it
//! whether it had — rendering the screen, running a detector over the result, sleeping ten
//! milliseconds, and asking again. That is correct and it is also the wrong shape, because the cost
//! of a wait then follows the CLOCK rather than the pane: a run standing at a permission dialog
//! waiting an hour for a person read that pane about 360,000 times, and every one of those reads
//! took the workspace lock every other client also reads through.
//!
//! Measured before this existed, through `sprag_plugin::testing::Counted`: a 400 ms wait for a
//! person cost **43** screen reads and a 1,600 ms wait cost **157** — 98 a second, which is
//! `POLL_INTERVAL` and nothing about the pane.
//!
//! # ⚠⚠⚠ The signal is the one the repaint seam already uses, and that is the point
//!
//! A pane's reader thread already announces *something changed* —
//! [`PaneHooks::on_dirty`](crate::pane_pty::PaneHooks::on_dirty), the
//! R999 repaint wake — at exactly three moments: a parsed batch was applied to the screen, the
//! child reached end of file, and the exit status arrived. **This counter is bumped at those same
//! three moments, in the same thread, beside the same call.** A second, independent notion of *the
//! pane moved* is a second answer that can drift; there is only one, and it is this.
//!
//! ⚠ A host that wires no `on_dirty` still gets the bumps. The hook is a caller's optional
//! interest in the event; the event is the pane's own.
//!
//! # ⚠⚠⚠⚠⚠ A FOURTH MOMENT, AND IT IS NOT A BYTE — register item 646, measured
//!
//! *The screen was written to* was this counter's whole definition for as long as everything a
//! waiter could ask about a pane was derived FROM the screen. That stopped being true when an
//! agent's own hook began reporting what it is doing: a turn ending arrives as a REPORT, out of
//! band, and changes what a supervisor reads about the pane **with no byte reaching it at all**.
//!
//! A waiter told *this verdict stands until the pane moves* then parked on a counter that could not
//! move, and the wait ran to its own bound reporting *the peer never finished* about a peer that
//! had finished. Measured 2026-08-24 through a real daemon: the driver's contract was armed at
//! `Working seq=1 asked_seq=1`, the daemon came to hold `Idle seq=2 asked_seq=2`, and the wait
//! answered `NotYet`. Shipped, that bound is half an hour a turn.
//!
//! So the counter's contract is **not** *the screen was written to*; it is
//! **[a permission to look](PaneRevision::await_after) — something a reader could see about this
//! pane has changed**, and the daemon's supervisor bumps it when it publishes a verdict. That is a
//! bump from a thread other than the reader's, which the mutex and condvar here already carry.
//!
//! ⚠⚠ The alternative — a SECOND counter for verdicts — is the thing the paragraph above refuses
//! by name, and the daemon's own revision pass refuses it again in as many words: *"a second notion
//! of `the pane moved` could tell this pass to answer while the slot said nothing had happened"*.
//! One counter, every reason.
//!
//! # ⚠⚠ What a waiter gets, and what it must still do for itself
//!
//! [`PaneRevision::await_after`] parks until the number passes the one the caller last saw, or
//! until its bound elapses. It answers the number NOW, so a caller that was woken and a caller that
//! timed out are told apart by comparing it — never by the return of the wait, which cannot say
//! whether a change arrived in the same instant the bound did.
//!
//! ⚠⚠⚠ **IT IS A PERMISSION TO LOOK, NOT AN ANSWER.** The number says something a reader could see
//! about this pane has changed; it says nothing about whether what the caller is waiting for has
//! become true. Every caller still evaluates its own predicate — this only decides *when it is
//! worth evaluating*.
//!
//! ⚠⚠ **AND A CALLER MUST STILL BOUND ITS OWN PARK.** Waiting here answers nothing about a
//! cancelled run or an expired deadline, which are facts about the RUN and not about the pane. A
//! caller that parked for an hour on a pane would be deaf to both for an hour; the callers in
//! `sprag-plugin` park in slices for exactly that reason, and the slices are cheap because a slice
//! that ends in a timeout reads no screen.

use std::sync::{Condvar, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

/// A pane's monotonic *moved* counter and the [`Condvar`] its reader thread notifies.
///
/// Shared between the pane and every handle to it, so a waiter holding a handle parks without
/// taking the workspace lock at all — see the module doc for why that matters as much as the count.
#[derive(Debug, Default)]
pub struct PaneRevision {
    /// How many times this pane has moved. Monotonic; never reset, never rolled back.
    ///
    /// ⚠ Starts at zero, and zero is a legitimate *seen* value: a caller that has looked at nothing
    /// yet passes `0` and is woken by the pane's very first batch.
    moved: Mutex<u64>,
    /// Notified after every bump, so a parked waiter wakes on the change rather than on a clock.
    changed: Condvar,
}

impl PaneRevision {
    /// A pane that has not moved yet.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// **THE PANE MOVED** — called by the reader thread beside its repaint wake, and by the
    /// daemon's supervisor when it publishes a verdict nobody typed a byte for (item 646).
    ///
    /// ⚠⚠⚠ **A CALLER THAT ALSO ANNOUNCES MUST BUMP FIRST.** The announce is what sends the pass
    /// that re-evaluates parked waits, and that pass compares this number against what each waiter
    /// last saw. Announce first and the pass finds the old number, answers nobody, and the waiter
    /// sleeps until some unrelated change moves the pane — the lost wakeup this counter exists to
    /// close, re-entered through the door marked *ordering*.
    ///
    /// ⚠ The lock is dropped BEFORE the notify. Notifying under the lock is correct and slower:
    /// every woken waiter would immediately block on the mutex the notifier still holds.
    ///
    /// ⚠⚠ [`Condvar::notify_all`] and not `notify_one`, because the waiters are independent readers
    /// of one fact — a repaint, a run's wait, a client's follow — and waking one of them would
    /// leave the others parked on a change that has already happened.
    pub fn bump(&self) {
        {
            let mut moved = lock(&self.moved);
            *moved = moved.wrapping_add(1);
        }
        self.changed.notify_all();
    }

    /// The pane's revision right now.
    ///
    /// ⚠ Cheap by construction: one uncontended mutex take and an integer read. No screen is
    /// rendered and no detector runs, which is the whole difference between this and a LOOK.
    #[must_use]
    pub fn now(&self) -> u64 {
        *lock(&self.moved)
    }

    /// **PARK UNTIL THIS PANE MOVES PAST `seen`**, or until `within` elapses — answering the
    /// revision as it stands on the way out.
    ///
    /// A caller compares the answer against what it passed: greater means the pane moved and it is
    /// worth looking again; equal means the bound elapsed and nothing happened.
    ///
    /// # ⚠⚠ Why the answer is the number and not a `bool`
    ///
    /// Because a change arriving in the same instant as the bound has to be reported as a change.
    /// A `timed_out` flag describes THIS wait's ending; the number describes the PANE, and it is
    /// the pane the caller is asking about. The condition is re-tested after every wake for the
    /// same reason the standard library requires it — a condvar may wake spuriously, and a wake
    /// this waiter did not cause is not evidence of anything.
    ///
    /// ⚠ `within` is bounded by subtraction from an elapsed time rather than by an absolute
    /// deadline: `Instant::now() + Duration::MAX` panics, and a caller passing an unbounded wait is
    /// asking a reasonable thing.
    #[must_use]
    pub fn await_after(&self, seen: u64, within: Duration) -> u64 {
        let began = Instant::now();
        let mut moved = lock(&self.moved);
        while *moved <= seen {
            let Some(left) = within.checked_sub(began.elapsed()) else {
                break;
            };
            if left.is_zero() {
                break;
            }
            let (guard, timed_out) = self
                .changed
                .wait_timeout(moved, left)
                .unwrap_or_else(PoisonError::into_inner);
            moved = guard;
            if timed_out.timed_out() {
                break;
            }
        }
        *moved
    }
}

/// A poisoned revision is not a corrupt one — the value behind it is a counter, and a panic while
/// holding it cannot leave it half-written. Recovering keeps a panicking reader from wedging every
/// waiter on the pane, which is this crate's rule everywhere else it holds a lock.
fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::thread;

    use super::*;

    /// ⚠⚠⚠ **A WAITER IS WOKEN BY THE CHANGE, NOT BY ITS BOUND** — the property the whole repair
    /// rests on, and the one that cannot be read off the code.
    ///
    /// The bound here is far longer than the change is away, so a wait that came back on the clock
    /// would be visible as elapsed time. ⚠ The control is the arm below it: with nothing bumping,
    /// the same call must spend its whole bound rather than returning at once — a park that never
    /// parks passes the first assertion and is the opposite defect.
    #[test]
    fn a_waiter_wakes_on_the_bump_and_not_on_the_bound() {
        const BOUND: Duration = Duration::from_secs(10);
        const SOON: Duration = Duration::from_millis(120);

        let revision = Arc::new(PaneRevision::new());
        assert_eq!(revision.now(), 0, "a pane that has not moved is at zero");

        let bumper = Arc::clone(&revision);
        let hand = thread::spawn(move || {
            thread::sleep(SOON);
            bumper.bump();
        });

        let began = Instant::now();
        let seen = revision.await_after(0, BOUND);
        let took = began.elapsed();
        hand.join().expect("the bumping thread");

        assert_eq!(
            seen, 1,
            "the wait must answer the revision AS IT STANDS, so a caller can tell a change from a \
             bound: got {seen}",
        );
        assert!(
            took < BOUND / 2,
            "⚠⚠⚠ THE WAIT CAME BACK ON ITS CLOCK. It took {took:?} of a {BOUND:?} bound for a \
             change that arrived after {SOON:?}, so nothing woke it and this is a sleep wearing a \
             park's name",
        );
    }

    /// ⚠⚠ **AND A PANE THAT DOES NOT MOVE COSTS THE WHOLE BOUND** — the control for the gate above,
    /// and the property a caller's own ceiling rests on.
    #[test]
    fn a_waiter_on_a_still_pane_spends_its_whole_bound() {
        const BOUND: Duration = Duration::from_millis(200);

        let revision = PaneRevision::new();
        let began = Instant::now();
        let seen = revision.await_after(0, BOUND);
        let took = began.elapsed();

        assert_eq!(
            seen, 0,
            "a pane nothing bumped must report the revision the caller already had",
        );
        assert!(
            took >= BOUND,
            "⚠⚠ a park that returns early on a still pane hands its caller back a poll: {took:?} \
             of {BOUND:?}",
        );
    }

    /// ⚠⚠⚠ **A CHANGE THAT ARRIVED BEFORE THE WAIT DID MUST NOT BE MISSED** — the lost-wakeup
    /// hazard, which is why the caller passes what it has SEEN rather than asking to be woken.
    ///
    /// A waiter that parked on *the next notify* would sleep out its whole bound here, having been
    /// told about the change before it asked. The number is what makes that impossible.
    #[test]
    fn a_change_that_happened_before_the_wait_answers_it_at_once() {
        const BOUND: Duration = Duration::from_secs(10);

        let revision = PaneRevision::new();
        revision.bump();
        revision.bump();

        let began = Instant::now();
        let seen = revision.await_after(0, BOUND);
        let took = began.elapsed();

        assert_eq!(seen, 2, "both bumps must be counted");
        assert!(
            took < BOUND / 2,
            "⚠⚠⚠ THE WAKE WAS LOST. The pane had already moved twice when the wait started and it \
             parked anyway for {took:?} — a caller that missed a change waits for the next one, \
             which on an idle pane never comes",
        );
    }
}
