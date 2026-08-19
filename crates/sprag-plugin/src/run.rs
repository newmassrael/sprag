//! `RunContext` — the per-run lifecycle context, and the one bounded-wait loop.
//!
//! The signals scoped to a *run* (not to a pane): cancellation today, a
//! deadline or human-approval gate later. Threaded into every [`Plugin::step`]
//! alongside `&dyn PaneAccess`, so the [`Driver`] depends on it for control and
//! `PaneAccess` stays the pane-scoped read/inject surface — each consumer
//! depends only on what it uses (interface segregation). This is the textbook
//! home cancellation belongs in; bolting it onto `PaneAccess` was the wrong
//! seam.
//!
//! [`poll_until`] is the single bounded, cancellable wait every plugin shares,
//! instead of three hand-rolled copies (the R12-R15 await loops).
//!
//! [`Plugin::step`]: crate::plugin::Plugin::step
//! [`Driver`]: crate::driver::Driver
//! [`PaneAccess`]: crate::access::PaneAccess

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::sleep;
use std::time::{Duration, Instant};

/// Poll interval for the bounded waits ([`poll_until`]).
pub const POLL_INTERVAL: Duration = Duration::from_millis(10);

/// Default overall bound on one AI reply — generous, since a real model thinks
/// for seconds. Shared by the [`Agent`](crate::agent) and
/// [`Dialogue`](crate::dialogue) adapters.
pub const DEFAULT_REPLY_TIMEOUT: Duration = Duration::from_secs(120);

/// The run-scoped lifecycle context handed to each [`Plugin::step`]. Carries
/// the two signals that end a run from outside its own logic: the cancel flag
/// (the host raises it) and the DEADLINE (the [`Driver`] arms it from
/// [`Guardrails::max_duration`]).
///
/// # ⚠⚠ Why the deadline lives here and not only in the guardrails
///
/// The guardrails are the DECLARATION; this is the armed clock. A ceiling the
/// Driver checked only at its loop top would bound how many steps a run takes
/// and not how long it takes, because a single step is free to block for its own
/// timeout — a dialogue turn waits [`DEFAULT_REPLY_TIMEOUT`] for a model to
/// think. A run asked to stop after one second would then stop after two
/// minutes, which is a bound in name only.
///
/// So the deadline reaches the WAITS: `poll_until` and the delivery path's own
/// loop both consult [`stopped`](Self::stopped), and a step in flight when the
/// deadline passes gives up at its next poll rather than at its own timeout.
///
/// [`Plugin::step`]: crate::plugin::Plugin::step
/// [`Driver`]: crate::driver::Driver
/// [`Guardrails::max_duration`]: crate::driver::Guardrails::max_duration
#[derive(Clone)]
pub struct RunContext {
    cancel: Arc<AtomicBool>,
    /// **WHETHER A PERSON HAS ASKED THIS RUN TO STAND DOWN** — beside the cancel flag because it
    /// arrives by the same route, and SEPARATE from it because it means the opposite thing.
    ///
    /// ⚠⚠⚠ A cancel says *stop now and throw the turn away*; this says *finish what you are doing
    /// and then stop*. Folding them into one flag would make the run that banked its milestone and
    /// the run that lost it indistinguishable to everything downstream — and those are the two
    /// outcomes a person is choosing between when they reach for either.
    ///
    /// ⚠ The flag only CARRIES the order. What it means is the loop document's, which holds it as a
    /// state in its own orders region and decides at the next milestone; nothing here judges.
    order: Arc<AtomicBool>,
    /// **WHETHER A PERSON HAS THIS RUN HELD** — the third thing somebody can say to it, and the
    /// only one they can take back.
    ///
    /// # ⚠⚠⚠⚠⚠ Why it is not [`order`](Self::order) with a second meaning
    ///
    /// `ai_loop.scxml` has carried *"a watching person can halt the loop between turns"* as an edge
    /// (`hold` → `awaiting_human`) since R378, **with nothing in the product able to raise it** —
    /// register item 9, and a transition no producer can take is the vacuous kind this workspace
    /// keeps paying for. What a person had instead were the two ENDINGS: `cancel` throws the turn
    /// away, `stand_down` finishes the milestone and converges. Neither is *wait, let me look*.
    ///
    /// ⚠⚠⚠ **AND IT IS TWO-WAY, WHICH IS WHY IT CANNOT SHARE `order`'s FLAG.** A stand-down is
    /// deliberately one-way — its doc says a *"stand down, no wait, carry on"* racing a milestone
    /// would make a run's ending depend on which message arrived first. A hold is the opposite by
    /// construction: the document's way back out is `resume`, and the run goes on. Folding the two
    /// into one flag would give the irreversible order an undo.
    ///
    /// ⚠ Like `order`, this only CARRIES the fact. What it MEANS is the document's — it is a state
    /// in the orders region and the machine decides — and nothing here judges.
    hold: Arc<AtomicBool>,
    /// WHEN THIS RUN MUST BE OVER, or [`None`] for a run nothing times.
    ///
    /// An `Instant` and not a `Duration`, because the question every wait asks is
    /// *"is it past yet?"* and a duration would need each of them to know when
    /// the run began. [`Driver::run`](crate::driver::Driver::run) stamps it from
    /// the guardrail at the top of the run, which is the one moment that answer
    /// is the same for every wait underneath it.
    deadline: Option<Instant>,
}

impl RunContext {
    /// A context backed by a host-shared cancel flag (set it to stop the run),
    /// with no deadline until one is [armed](Self::deadline_in).
    #[must_use]
    pub fn new(cancel: Arc<AtomicBool>) -> Self {
        Self {
            cancel,
            order: Arc::new(AtomicBool::new(false)),
            hold: Arc::new(AtomicBool::new(false)),
            deadline: None,
        }
    }

    /// This context sharing `order` — the flag a host raises when somebody tells the run to stand
    /// down. A context built without one can never be ordered, which is what every driver that has
    /// no host to be spoken through should be.
    ///
    /// ⚠ Separate from [`new`](Self::new) rather than a second parameter, so the many callers that
    /// only ever cancel keep the signature they had — and so a caller that DOES wire orders has
    /// written that down.
    #[must_use]
    pub fn ordered_by(self, order: Arc<AtomicBool>) -> Self {
        Self { order, ..self }
    }

    /// This context sharing `hold` — the flag a host raises and LOWERS when somebody halts the run
    /// and lets it go again. [`ordered_by`](Self::ordered_by)'s terms exactly, one order over: a
    /// driver with no host to be spoken through can never be held, which is what it should be.
    #[must_use]
    pub fn held_by(self, hold: Arc<AtomicBool>) -> Self {
        Self { hold, ..self }
    }

    /// A context that can never be cancelled — for fire-and-forget runs and
    /// tests that don't exercise cancellation.
    #[must_use]
    pub fn uncancellable() -> Self {
        Self::new(Arc::new(AtomicBool::new(false)))
    }

    /// This context with its deadline set `within` from NOW — what the
    /// [`Driver`](crate::driver::Driver) hands the plugin, sharing the same
    /// cancel flag.
    ///
    /// `None` disarms it, which is what a run with no time ceiling wants and what
    /// every caller that drives a plugin without a host gets by default.
    #[must_use]
    pub fn deadline_in(&self, within: Option<Duration>) -> Self {
        Self {
            cancel: Arc::clone(&self.cancel),
            // ⚠ CARRIED, like the cancel flag beside it. A derived context that dropped the order
            // would leave a run unable to hear the one thing a person said to it, and the drop
            // would be invisible — the run would simply carry on working.
            order: Arc::clone(&self.order),
            // CARRIED for `order`'s reason and one harder: a derived context that dropped this
            // would leave a HELD run driving on, which is the one order whose whole point is that
            // the run stops moving while somebody reads its pane.
            hold: Arc::clone(&self.hold),
            deadline: within.map(|d| Instant::now() + d),
        }
    }

    /// Whether the run has been asked to stop.
    #[must_use]
    pub fn cancelled(&self) -> bool {
        self.cancel.load(Ordering::Acquire)
    }

    /// **WHETHER SOMEBODY HAS ASKED THIS RUN TO FINISH UP AND STAND DOWN.**
    ///
    /// ⚠ Deliberately NOT part of [`stopped`](Self::stopped): a stood-down run is still running, and
    /// every wait that treated it as finished would abandon the turn this order exists to let it
    /// finish.
    #[must_use]
    pub fn stood_down(&self) -> bool {
        self.order.load(Ordering::Acquire)
    }

    /// **WHETHER A PERSON HAS THIS RUN HELD** — see the field for why it is neither a cancel nor a
    /// stand-down.
    ///
    /// ⚠⚠⚠ NOT part of [`stopped`](Self::stopped), for [`stood_down`](Self::stood_down)'s reason
    /// and more sharply: a held run is not finishing, it is WAITING, and a wait that read this as
    /// *the run is over* would abandon the very turn the person means to come back to.
    #[must_use]
    pub fn held(&self) -> bool {
        self.hold.load(Ordering::Acquire)
    }

    /// Whether the run's deadline has passed. Always false for a run with none.
    #[must_use]
    pub fn expired(&self) -> bool {
        self.deadline.is_some_and(|at| Instant::now() >= at)
    }

    /// Whether the run is OVER — cancelled, or out of time.
    ///
    /// The predicate every bounded wait consults, so neither of the two ways a
    /// run ends from outside can be honoured by one wait and missed by another.
    /// Which of the two it was is the context's to answer
    /// ([`cancelled`](Self::cancelled) / [`expired`](Self::expired)) and not the
    /// wait's, so the reason has ONE authority.
    #[must_use]
    pub fn stopped(&self) -> bool {
        self.cancelled() || self.expired()
    }
}

/// How a bounded wait ([`poll_until`]) ended.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Waited {
    /// The predicate became true.
    Ready,
    /// THE RUN ITSELF ENDED mid-wait — cancelled, or past its deadline. Ask the
    /// [`RunContext`] which; the wait does not hold a second copy of that fact.
    ///
    /// ⚠ This variant replaced a `Cancelled` one when the deadline was added, and
    /// the rename is the point rather than a tidy-up: every site that compared
    /// against `Waited::Cancelled` would have kept compiling with a new variant
    /// beside it and would have read a deadline as *keep going* — the exact class
    /// of silent unbounding this whole ceiling exists to close. A renamed variant
    /// fails to compile at each of them instead.
    Stopped,
    /// The timeout elapsed before the predicate held.
    TimedOut,
}

/// Wait until `predicate` holds, bounded by `timeout` AND by the run's own
/// deadline, pre-empted by cancel.
///
/// Polls `predicate` every [`POLL_INTERVAL`], returning [`Waited::Stopped`] the
/// moment the run is cancelled or out of time (so a long in-flight AI turn
/// aborts promptly), [`Waited::Ready`] when the predicate holds, or
/// [`Waited::TimedOut`] when the local bound elapses. The one bounded wait the
/// adapters share — they differ only in their predicate.
///
/// ⚠ The ORDER is load-bearing twice. Cancel is asked first, so a person's stop
/// beats a predicate that happens to come true in the same instant. And READY is
/// asked before the deadline, so work that finished is never thrown away by a
/// clock that ran out while it was finishing — the run ends at the Driver's next
/// loop top either way, one step later at most.
pub fn poll_until(
    run: &RunContext,
    timeout: Duration,
    mut predicate: impl FnMut() -> bool,
) -> Waited {
    let start = Instant::now();
    loop {
        if run.cancelled() {
            return Waited::Stopped;
        }
        if predicate() {
            return Waited::Ready;
        }
        if run.expired() {
            return Waited::Stopped;
        }
        if start.elapsed() >= timeout {
            return Waited::TimedOut;
        }
        sleep(POLL_INTERVAL);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// ⚠⚠⚠⚠ **THE ORDER THIS FILE CALLS LOAD-BEARING, HELD** — both halves, on the one wait every
    /// plugin in this crate shares.
    ///
    /// [`poll_until`]'s doc says the order is load-bearing twice and nothing asked it. The gates
    /// that exercise this function do it through real panes, and none of them can stage the two
    /// cases the order is ABOUT, because both need the predicate and the ending to be true AT ONCE:
    /// a read-back that raises a cancel leaves the predicate false, so it proves the flag is
    /// consulted and not that it is consulted FIRST.
    ///
    /// ⚠⚠ It is also the sentence a shutdown's deadline rests on one level up — *a run hears cancel
    /// inside every bounded wait it takes* — so a swap here would lengthen how long a worker takes
    /// to come back without a single gate moving.
    #[test]
    fn a_wait_answers_the_endings_in_the_order_its_doc_promises() {
        // ⚠ CANCEL BEATS A PREDICATE THAT IS ALREADY TRUE. A person's stop landing in the same
        // instant as the thing they were waiting for must not be read as *carry on*: the work is
        // over, and whatever the screen says about it is the next caller's business.
        let cancel = Arc::new(AtomicBool::new(true));
        assert_eq!(
            poll_until(&RunContext::new(cancel), Duration::from_secs(30), || true),
            Waited::Stopped,
            "a cancelled run whose predicate is true must report the ENDING, not the predicate",
        );

        // ⚠ AND READY BEATS A DEADLINE THAT HAS ALREADY PASSED, which is the same rule pointed the
        // other way: work that finished must never be thrown away by a clock that ran out while it
        // was finishing. The run still ends — at the Driver's next loop top, one step later at
        // most — but it ends having KEPT what it just did.
        let expired = RunContext::uncancellable().deadline_in(Some(Duration::ZERO));
        assert!(
            expired.expired(),
            "the fixture must stage a passed deadline"
        );
        assert_eq!(
            poll_until(&expired, Duration::from_secs(30), || true),
            Waited::Ready,
            "a finished predicate must survive a deadline that passed while it was finishing",
        );

        // ⚠ And the third arm still answers for itself: nothing true, no ending, no time.
        assert_eq!(
            poll_until(&RunContext::uncancellable(), Duration::ZERO, || false),
            Waited::TimedOut,
        );
    }

    /// ⚠⚠⚠⚠ **A STOOD-DOWN RUN IS STILL RUNNING, AND EVERY WAIT MUST GO ON WAITING** — the one
    /// invariant that makes *finish what you are doing and then stop* different from *stop*.
    ///
    /// [`RunContext::stood_down`] is deliberately not part of [`RunContext::stopped`], and its doc
    /// says why: a wait that treated the order as an ending would abandon the very turn the order
    /// exists to let a run finish, and the two outcomes — the run that banked its milestone and the
    /// run that lost it — are exactly what a person is choosing between when they reach for one
    /// word rather than the other. Folding them together compiles, keeps every other gate green,
    /// and silently turns `stand_down` into a slower `cancel`.
    ///
    /// ⚠⚠⚠⚠ **AND NOTHING ELSE HELD IT, MEASURED**: with `stood_down` folded into `stopped`, the
    /// whole crate's other 320 gates stay green and only this one goes red. The word shipped with
    /// its wire, its CLI verb and its document state, and the one invariant separating it from
    /// `cancel` was defended by nobody.
    #[test]
    fn an_order_to_stand_down_does_not_end_a_wait() {
        let order = Arc::new(AtomicBool::new(true));
        let run = RunContext::uncancellable().ordered_by(Arc::clone(&order));
        assert!(run.stood_down(), "the fixture must stage the order");
        assert!(
            !run.stopped(),
            "a stood-down run is still running, so nothing may read it as over",
        );
        assert_eq!(
            poll_until(&run, Duration::from_millis(30), || false),
            Waited::TimedOut,
            "the wait must run to its own bound, not report the order as an ending",
        );
    }

    /// ⚠⚠⚠⚠⚠ **A HELD RUN IS NEITHER OVER NOR STOOD DOWN, AND IT CAN BE LET GO** — register item 9,
    /// and the three claims that keep `hold` from collapsing into either of its neighbours.
    ///
    /// A hold folded into [`stopped`](RunContext::stopped) would end every wait the moment somebody
    /// paused a run, which is a cancel with a friendlier word. Folded into
    /// [`stood_down`](RunContext::stood_down) it would converge the run at its next milestone, which
    /// is the other ending. **The whole point is that the run is still there when the person looks
    /// up** — so this asserts what a hold is NOT, and then the one thing only it can do.
    ///
    /// ⚠⚠ **THE REVERSAL IS THE THIRD ASSERTION AND THE SHARPEST.** `order` is a latch by design;
    /// this must not be, or the document's `resume` edge has nothing to be raised by and `hold`
    /// becomes a slow cancel. A flag that could only be set would pass the first two.
    #[test]
    fn a_held_run_is_not_over_not_stood_down_and_can_be_let_go() {
        let hold = Arc::new(AtomicBool::new(true));
        let run = RunContext::uncancellable().held_by(Arc::clone(&hold));
        assert!(run.held(), "the fixture must stage the order");
        assert!(
            !run.stopped(),
            "⚠⚠⚠⚠⚠ A HELD RUN IS WAITING, NOT FINISHED. Folded into `stopped`, every wait in this \
             crate ends the moment somebody pauses a run — which is `cancel` wearing a kinder word, \
             and the turn the person meant to come back to is gone",
        );
        assert!(
            !run.stood_down(),
            "⚠⚠⚠⚠ AND IT IS NOT THE OTHER ENDING EITHER. A hold read as a stand-down converges the \
             run at its next milestone, so a person who asked to READ a pane would find the loop \
             finished when they looked up",
        );
        assert_eq!(
            poll_until(&run, Duration::from_millis(30), || false),
            Waited::TimedOut,
            "and the waits run to their own bounds, exactly as they do under an order",
        );

        // ── THE ARM ITS NEIGHBOURS CANNOT HAVE: the person lets go. ──
        hold.store(false, Ordering::Release);
        assert!(
            !run.held(),
            "⚠⚠⚠⚠⚠ THE ONE ORDER A PERSON CAN TAKE BACK. `order` is a latch on purpose — an \
             un-ordering racing a milestone would make a run's ending depend on which message \
             arrived first — and this is the opposite by construction: the document's way back out \
             is `resume`, and a flag that could only be set would leave that edge with nothing to \
             raise it",
        );
    }
}
